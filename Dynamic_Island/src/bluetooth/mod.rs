//! Paired Bluetooth devices (name, connected, battery) read through PowerShell.
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use slint::{ModelRc, VecModel, Weak};

use crate::{BtItem, MainWindow};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static WANTED: AtomicBool = AtomicBool::new(false);
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn set_wanted(on: bool) {
    let was = WANTED.swap(on, Ordering::Relaxed);
    if on && !was {
        REFRESH.store(true, Ordering::Relaxed);
    }
}

pub fn refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

const SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue'
Get-PnpDevice -Class Bluetooth | Where-Object { $_.InstanceId -like 'BTHENUM\DEV_*' -or $_.InstanceId -like 'BTHLE\DEV_*' } | ForEach-Object {
  $c = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName '{83DA6326-97A6-4088-9453-A1923F573B29} 15').Data
  $b = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName '{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2').Data
  if ($b -eq $null) { $b = -1 }
  Write-Output ("{0}|{1}|{2}" -f $_.FriendlyName, $c, $b)
}
"#;

#[derive(Clone, PartialEq)]
struct Dev {
    name: String,
    connected: bool,
    battery: i32,
}

fn read_devices() -> Vec<Dev> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut list: Vec<Dev> = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.trim().split('|').collect();
        if parts.len() < 3 || parts[0].trim().is_empty() {
            continue;
        }
        let name = parts[0].trim().to_string();
        let connected = parts[1].trim().eq_ignore_ascii_case("true");
        let battery = parts[2].trim().parse::<i32>().unwrap_or(-1);
        if let Some(existing) = list.iter_mut().find(|d| d.name == name) {
            existing.connected |= connected;
            if battery >= 0 {
                existing.battery = battery;
            }
        } else {
            list.push(Dev { name, connected, battery });
        }
    }
    // connected ones first
    list.sort_by_key(|d| !d.connected);
    list
}

pub fn spawn(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        let mut last: Option<Vec<Dev>> = None;
        let mut last_read = Instant::now().checked_sub(Duration::from_secs(60)).unwrap_or_else(Instant::now);
        loop {
            thread::sleep(Duration::from_millis(400));
            if !WANTED.load(Ordering::Relaxed) {
                continue;
            }
            let forced = REFRESH.swap(false, Ordering::Relaxed);
            if !forced && last_read.elapsed() < Duration::from_secs(8) {
                continue;
            }
            last_read = Instant::now();
            let devices = read_devices();
            if last.as_ref() == Some(&devices) {
                continue;
            }
            last = Some(devices.clone());

            let sent = ui.upgrade_in_event_loop(move |ui| {
                let first = devices.iter().find(|d| d.connected).cloned();
                let items: Vec<BtItem> = devices
                    .iter()
                    .map(|d| BtItem {
                        name: d.name.clone().into(),
                        letter: crate::notifications::letter_of(&d.name).into(),
                        connected: d.connected,
                        battery: d.battery,
                    })
                    .collect();
                ui.set_bt_devices(ModelRc::new(VecModel::from(items)));
                match first {
                    Some(d) => {
                        ui.set_bt_name(d.name.into());
                        ui.set_bt_battery(d.battery);
                    }
                    None => {
                        ui.set_bt_name("".into());
                        ui.set_bt_battery(-1);
                    }
                }
            });
            if sent.is_err() {
                break;
            }
        }
    });
}
