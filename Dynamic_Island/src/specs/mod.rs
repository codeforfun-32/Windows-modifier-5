use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use slint::Weak;
use sysinfo::{Disks, Networks, System};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::MainWindow;

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// How often CPU, RAM, storage and battery are measured.
const BASIC_EVERY: Duration = Duration::from_secs(5);
/// How often the network speed is measured (averaged over this time).
const NET_EVERY: Duration = Duration::from_secs(10);

/// Sampling only runs while the Specs tab is open or pinned to the pill.
static WANTED: AtomicBool = AtomicBool::new(false);

pub fn set_wanted(on: bool) {
    WANTED.store(on, Ordering::Relaxed);
}

fn speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1_048_576.0)
    } else if bytes_per_sec >= 1024.0 {
        format!("{:.0} KB/s", bytes_per_sec / 1024.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// (charge 0..1, plugged in, has battery)
fn battery() -> (f32, bool, bool) {
    let mut status = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut status).is_ok() };
    if !ok {
        return (0.0, false, false);
    }
    let present = status.BatteryFlag != 128 && status.BatteryLifePercent != 255;
    (status.BatteryLifePercent as f32 / 100.0, status.ACLineStatus == 1, present)
}

fn net_totals() -> (u64, u64) {
    let networks = Networks::new_with_refreshed_list();
    let mut rx = 0u64;
    let mut tx = 0u64;
    for (name, data) in networks.iter() {
        if name.to_lowercase().contains("loopback") {
            continue;
        }
        rx += data.total_received();
        tx += data.total_transmitted();
    }
    (rx, tx)
}

fn storage() -> (String, String, f32) {
    let disks = Disks::new_with_refreshed_list();
    let pick = disks
        .iter()
        .find(|d| d.mount_point().to_string_lossy().to_uppercase().starts_with("C:"))
        .or_else(|| disks.iter().next());
    match pick {
        Some(d) => {
            let total = d.total_space() as f64;
            let free = d.available_space() as f64;
            let used = if total > 0.0 { ((total - free) / total) as f32 } else { 0.0 };
            (
                format!("{:.1} GB free", free / GB),
                format!("of {:.0} GB", total / GB),
                used.clamp(0.0, 1.0),
            )
        }
        None => ("--".to_string(), String::new(), 0.0),
    }
}

fn uptime_text() -> String {
    let s = System::uptime();
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

pub fn spawn(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        let mut sys = System::new();
        let mut net_base: Option<(u64, u64, Instant)> = None;
        let mut last_basic: Option<Instant> = None;
        let mut active = false;

        // the numbers on screen: they only change when a new measurement is taken
        let mut cpu = 0.0f32;
        let mut ram = 0.0f32;
        let mut ram_text = String::from("--");
        let mut ram_total_text = String::new();
        let mut storage_text = String::from("--");
        let mut storage_total = String::new();
        let mut storage_used = 0.0f32;
        let mut uptime = String::from("--");
        let mut hist: Vec<f32> = Vec::new();
        let mut batt = 0.0f32;
        let mut charging = false;
        let mut present = false;
        let mut down_text = String::from("--");
        let mut up_text = String::from("--");

        loop {
            if !WANTED.load(Ordering::Relaxed) {
                active = false;
                net_base = None;
                last_basic = None;
                thread::sleep(Duration::from_millis(300));
                continue;
            }

            if !active {
                // just switched on: start the network clock and warm up the CPU counter
                active = true;
                sys.refresh_cpu_usage();
                let (rx, tx) = net_totals();
                net_base = Some((rx, tx, Instant::now()));
                thread::sleep(Duration::from_millis(1500));
            }

            let now = Instant::now();
            let mut changed = false;

            // network speed: every NET_EVERY, averaged over that time
            if let Some((prx, ptx, t)) = net_base {
                if now.duration_since(t) >= NET_EVERY {
                    let (rx, tx) = net_totals();
                    let dt = now.duration_since(t).as_secs_f64().max(0.001);
                    down_text = speed(rx.saturating_sub(prx) as f64 / dt);
                    up_text = speed(tx.saturating_sub(ptx) as f64 / dt);
                    net_base = Some((rx, tx, now));
                    changed = true;
                }
            }

            // cpu, memory, gpu, battery: every BASIC_EVERY
            let basic_due = last_basic.map_or(true, |t| now.duration_since(t) >= BASIC_EVERY);
            if basic_due {
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                cpu = (sys.global_cpu_usage() / 100.0).clamp(0.0, 1.0);
                let total = sys.total_memory() as f64;
                let used = sys.used_memory() as f64;
                ram = if total > 0.0 { (used / total) as f32 } else { 0.0 };
                ram_text = format!("{:.1} GB", used / GB);
                ram_total_text = format!("of {:.1} GB", total / GB);

                hist.push(cpu);
                if hist.len() > 12 {
                    hist.remove(0);
                }
                let (st, stt, stu) = storage();
                storage_text = st;
                storage_total = stt;
                storage_used = stu;
                uptime = uptime_text();

                let (b, c, p) = battery();
                batt = b;
                charging = c;
                present = p;

                last_basic = Some(now);
                changed = true;
            }

            if changed {
                // compact text for the collapsed pill
                let mut text = format!("CPU {:.0}%", cpu * 100.0);
                text.push_str(&format!("  RAM {:.0}%", ram * 100.0));

                let ram_text = ram_text.clone();
                let down_text = down_text.clone();
                let up_text = up_text.clone();
                let ram_total_text = ram_total_text.clone();
                let storage_text = storage_text.clone();
                let storage_total = storage_total.clone();
                let uptime = uptime.clone();
                let hist = hist.clone();
                let sent = ui.upgrade_in_event_loop(move |ui| {
                    ui.set_spec_cpu(cpu);
                    ui.set_spec_ram(ram);
                    ui.set_spec_ram_text(ram_text.into());
                    ui.set_spec_ram_total(ram_total_text.into());
                    ui.set_spec_storage_text(storage_text.into());
                    ui.set_spec_storage_total(storage_total.into());
                    ui.set_spec_storage_used(storage_used);
                    ui.set_spec_uptime(uptime.into());
                    ui.set_spec_hist(slint::ModelRc::new(slint::VecModel::from(hist)));
                    ui.set_spec_down(down_text.into());
                    ui.set_spec_up(up_text.into());
                    ui.set_spec_batt(batt);
                    ui.set_spec_charging(charging);
                    ui.set_spec_batt_present(present);
                    ui.set_spec_text(text.into());
                });
                if sent.is_err() {
                    break;
                }
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}