#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod media;
mod native;
mod video;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use shared::Config;
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize, Timer, TimerMode};
use windows::Win32::Foundation::HWND;

const MAX_ATTACH_TRIES: u32 = 20;

struct State {
    cfg: Config,
    player: Option<media::Player>,
    key: String,
    hwnd: Option<HWND>,
    embedded: bool,
    visible: bool,
    has_frame: bool,
    paused: bool,
    screen: (i32, i32),
    attach_tries: u32,
    status: String,
    last_cfg: Instant,
    last_pause: Instant,
    last_attach: Instant,
    next_frame: Instant,
}

fn write_status(s: &mut State, text: &str) {
    if s.status == text {
        return;
    }
    s.status = text.to_string();
    let _ = std::fs::create_dir_all(Config::dir());
    let _ = std::fs::write(Config::dir().join("wallpaper_status.txt"), format!("{text}\r\n"));
}

fn fps_limit(cfg: &Config) -> u64 {
    let n: u64 = cfg.get("wp_fps").parse().unwrap_or(30);
    n.clamp(5, 120)
}

fn fit_number(cfg: &Config) -> i32 {
    match cfg.get("wp_fit").as_str() {
        "Fit" => 1,
        "Stretch" => 2,
        "Center" => 3,
        _ => 0,
    }
}

/// The list the user typed in settings, as lower-case program names like "chrome.exe".
fn app_list(cfg: &Config) -> Vec<String> {
    cfg.get("wp_pause_apps")
        .split(|c: char| c == ',' || c == ';' || c == '\n' || c == '\r')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .map(|s| if s.contains('.') { s } else { format!("{s}.exe") })
        .collect()
}

/// Should the wallpaper stand still right now?
fn should_pause(cfg: &Config) -> bool {
    if cfg.on("wp_pause_battery") && native::on_battery() {
        return true;
    }
    match cfg.get("wp_pause").as_str() {
        "App maximized" => {
            let (sw, sh) = native::screen();
            native::app_windows(false).iter().any(|a| {
                let (l, t, r, b) = a.rect;
                (r - l) >= sw * 95 / 100 && (b - t) >= sh * 85 / 100
            })
        }
        "Fullscreen app" => native::fullscreen_app(),
        "Any app open" => !native::app_windows(false).is_empty(),
        "Selected apps" => {
            let list = app_list(cfg);
            if list.is_empty() {
                return false;
            }
            native::app_windows(true).iter().any(|a| list.contains(&a.exe))
        }
        _ => false,
    }
}

/// Opens the chosen file (or clears the wallpaper when there is nothing to show).
fn reload(s: &mut State, ui: &WallWindow) {
    s.player = None;
    s.has_frame = false;
    ui.set_has_frame(false);

    if s.cfg.get("wp_type") == "Interactive" {
        write_status(s, "Interactive wallpapers are coming in a later version.");
        return;
    }
    let path = s.cfg.get("wp_path");
    let flip = s.cfg.on("wp_flip");
    match media::load(&path, flip) {
        Ok((player, first)) => {
            if let Some(img) = first {
                ui.set_frame(img);
                ui.set_has_frame(true);
                s.has_frame = true;
            }
            s.player = Some(player);
            s.next_frame = Instant::now();
            write_status(s, &format!("Playing: {path}"));
        }
        Err(e) => write_status(s, &e),
    }
}

/// Reads the settings and applies them. Runs about once a second.
fn apply_settings(s: &mut State, ui: &WallWindow) {
    s.cfg.sync();

    if !s.cfg.on("wp_enabled") {
        let _ = slint::quit_event_loop();
        return;
    }

    ui.set_fit(fit_number(&s.cfg));
    let dim = s.cfg.num("wp_dim").clamp(0, 80) as f32 / 100.0;
    ui.set_dim(dim);

    let key = format!("{}|{}|{}", s.cfg.get("wp_type"), s.cfg.get("wp_path"), s.cfg.get("wp_flip"));
    if key != s.key {
        s.key = key;
        reload(s, ui);
    }

    // a video that cannot be played
    let failed = s.player.as_ref().and_then(|p| p.error());
    if let Some(e) = failed {
        s.player = None;
        s.has_frame = false;
        ui.set_has_frame(false);
        write_status(s, &e);
    }
}

/// Finds our window, attaches it to the desktop layer and keeps it the size of the screen.
fn keep_attached(s: &mut State, ui: &WallWindow, now: Instant) {
    if s.hwnd.is_none() {
        if let Some(h) = native::hwnd_of(ui.window()) {
            native::sync_styles(h);
            native::set_visible(h, false);
            s.hwnd = Some(h);
        }
    }
    let Some(h) = s.hwnd else { return };

    let screen = native::screen();
    if s.embedded {
        if screen != s.screen {
            s.screen = screen;
            native::fit_to_screen(h, screen.0, screen.1);
        }
        return;
    }
    if s.attach_tries >= MAX_ATTACH_TRIES {
        return;
    }
    if now.duration_since(s.last_attach) < Duration::from_millis(700) {
        return;
    }
    s.last_attach = now;
    s.attach_tries += 1;
    s.screen = screen;
    if native::embed(h, screen.0, screen.1) {
        s.embedded = true;
    } else if s.attach_tries >= MAX_ATTACH_TRIES {
        write_status(s, "Could not attach the wallpaper to the desktop on this Windows version.");
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // only one wallpaper at a time
    if shared::process_count("wallpaper-app.exe") > 1 {
        return Ok(());
    }

    let ui = WallWindow::new()?;
    let (sw, sh) = native::screen();
    ui.window().set_size(PhysicalSize::new(sw.max(200) as u32, sh.max(200) as u32));
    ui.window().set_position(PhysicalPosition::new(0, 0));
    ui.show()?;

    let now = Instant::now();
    let past = now.checked_sub(Duration::from_secs(10)).unwrap_or(now);
    let state = Rc::new(RefCell::new(State {
        cfg: Config::load(),
        player: None,
        key: String::from("\u{1}"),
        hwnd: None,
        embedded: false,
        visible: false,
        has_frame: false,
        paused: false,
        screen: (sw, sh),
        attach_tries: 0,
        status: String::new(),
        last_cfg: past,
        last_pause: past,
        last_attach: past,
        next_frame: now,
    }));

    let timer = Timer::default();
    {
        let weak = ui.as_weak();
        let state = state.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(15), move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut guard = state.borrow_mut();
            let s = &mut *guard;
            let now = Instant::now();

            // settings, about once a second
            if now.duration_since(s.last_cfg) >= Duration::from_millis(1000) {
                s.last_cfg = now;
                apply_settings(s, &ui);
            }

            // attach to the desktop
            keep_attached(s, &ui, now);

            // pause rules, a few times a second
            if now.duration_since(s.last_pause) >= Duration::from_millis(600) {
                s.last_pause = now;
                s.paused = should_pause(&s.cfg);
            }

            // new picture (video and GIF), no faster than the frame limit
            if now >= s.next_frame {
                let fps = fps_limit(&s.cfg);
                s.next_frame = now + Duration::from_millis(1000 / fps);
                let paused = s.paused;
                if let Some(p) = s.player.as_mut() {
                    if let Some(img) = p.tick(paused) {
                        ui.set_frame(img);
                        ui.set_has_frame(true);
                        s.has_frame = true;
                    }
                }
            }

            // show the window only when it is attached and has something to show
            let want = s.embedded && s.has_frame;
            if want != s.visible {
                if let Some(h) = s.hwnd {
                    native::set_visible(h, want);
                    s.visible = want;
                }
            }
        });
    }

    let result = slint::run_event_loop_until_quit();
    drop(timer);
    drop(ui);
    result
}
