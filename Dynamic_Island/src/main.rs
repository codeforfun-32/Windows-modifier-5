#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod audio;
mod bluetooth;
mod calendar;
mod clipboard;
mod companion;
mod links;
mod look;
mod media;
mod notifications;
mod quick;
mod sites;
mod specs;
mod ticker;
mod todo;
mod tools;
mod weather;
mod webhook;
mod win32;

slint::include_modules!();

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use shared::Config;
use slint::{ComponentHandle, Model, Timer, TimerMode, VecModel};
use windows::Win32::Foundation::HWND;

use companion::eyes::Eyes;
use companion::state_machine::{Companion, HoverTracker};

fn sync_bars(model: &VecModel<f32>, levels: &[f32; audio::BARS]) {
    for (i, v) in levels.iter().enumerate() {
        let current = model.row_data(i).unwrap_or(0.0);
        if (current - *v).abs() > 0.002 {
            model.set_row_data(i, *v);
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // only one island at a time
    if shared::process_count("dynamic-island.exe") > 1 {
        return Ok(());
    }
    let config = Rc::new(RefCell::new(Config::load()));
    if !config.borrow().on("island_enabled") {
        return Ok(());
    }

    let ui = MainWindow::new()?;
    let weak = ui.as_weak();
    look::apply(&ui, &config.borrow());
    ui.set_page(look::default_page(&config.borrow()));
    calendar::start_clock(&ui);
    todo::init(&ui);

    let viz_model = Rc::new(VecModel::from(vec![0.0f32; audio::BARS]));
    ui.set_viz(viz_model.clone().into());

    let viz = audio::Meter::spawn(audio::Source::Loopback, 5.0);
    let companion = Rc::new(RefCell::new(Companion::new()));
    let hwnd_cell: Rc<Cell<Option<HWND>>> = Rc::new(Cell::new(None));

    // ---------- music ----------
    ui.on_media_prev(media::previous);
    ui.on_media_toggle(media::toggle);
    ui.on_media_next(media::next);
    ui.on_seek(media::seek);
    {
        let weak = weak.clone();
        ui.on_source_picked(move |key| {
            media::select(key.as_str());
            if let Some(ui) = weak.upgrade() {
                ui.set_source_menu(false);
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_title_tapped(move || {
            if let Some(ui) = weak.upgrade() {
                links::open_now_playing(
                    ui.get_source_name().to_string(),
                    ui.get_track_title().to_string(),
                    ui.get_track_artist().to_string(),
                );
            }
        });
    }

    // ---------- clipboard ----------
    {
        let weak = weak.clone();
        ui.on_copy_clip(move |id| {
            if clipboard::copy(id.max(0) as usize) {
                ticker::push(&weak, "Copied to clipboard".to_string());
            }
        });
    }
    { let weak = weak.clone(); ui.on_clip_remove(move |id| clipboard::remove(&weak, id.max(0) as usize)); }
    { let weak = weak.clone(); ui.on_clip_pin(move |id| clipboard::toggle_pin(&weak, id.max(0) as usize)); }
    { let weak = weak.clone(); ui.on_clip_clear(move || clipboard::clear(&weak)); }
    { let weak = weak.clone(); ui.on_clip_filter_changed(move |f| clipboard::set_filter(&weak, f)); }

    // ---------- sites, notifications, calendar ----------
    { let weak = weak.clone(); ui.on_site_add(move || sites::add_from_clipboard(&weak)); }
    ui.on_site_open(|index| sites::open(index.max(0) as usize));
    { let weak = weak.clone(); ui.on_site_remove(move |index| sites::remove(&weak, index.max(0) as usize)); }
    { let weak = weak.clone(); ui.on_notifs_clear(move || notifications::clear(&weak)); }
    {
        let weak = weak.clone();
        ui.on_cal_step(move |delta| {
            if let Some(ui) = weak.upgrade() {
                calendar::step(&ui, delta);
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_cal_reset(move || {
            if let Some(ui) = weak.upgrade() {
                calendar::show_today(&ui);
            }
        });
    }

    // ---------- window buttons ----------
    ui.on_open_settings(|| {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let _ = std::process::Command::new(dir.join("settings-app.exe")).spawn();
            }
        }
    });
    ui.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });
    let suppress: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let suppress = suppress.clone();
        ui.on_close_island(move || suppress.set(true));
    }
    {
        let hwnd_cell = hwnd_cell.clone();
        ui.on_text_focus(move |on| {
            if let Some(h) = hwnd_cell.get() {
                win32::window::set_text_focus(h, on);
            }
        });
    }

    // ---------- quick settings ----------
    {
        let q = ui.global::<QuickState>();
        q.on_set_wifi(|on| quick::send(quick::Cmd::Wifi(on)));
        q.on_set_bluetooth(|on| quick::send(quick::Cmd::Bluetooth(on)));
        q.on_set_airplane(|on| quick::send(quick::Cmd::Airplane(on)));
        q.on_toggle_mute(|| quick::send(quick::Cmd::Mute));
        q.on_set_volume(|v| quick::send(quick::Cmd::Volume(v)));
        q.on_set_brightness(|v| quick::send(quick::Cmd::Brightness(v)));
        q.on_open_setting(|uri| quick::open_setting(uri.as_str()));
    }

    // ---------- focus timer, tasks ----------
    {
        let companion = companion.clone();
        let weak = weak.clone();
        ui.on_timer_finished(move |mode| {
            let Some(ui) = weak.upgrade() else { return };
            let (title, body) = if mode == 0 {
                ("Focus complete", "Nice work. Time for a break.")
            } else {
                ("Break over", "Ready for another round?")
            };
            ticker::toast(&ui, "Focus", title, body);
            let m = companion.borrow_mut().cheer(Instant::now());
            ui.set_mood(m.as_int());
            audio::play_sfx("done");
        });
    }
    { let weak = weak.clone(); ui.on_task_add(move |t| todo::add(&weak, t.as_str())); }
    { let weak = weak.clone(); ui.on_task_toggle(move |i| todo::toggle(&weak, i.max(0) as usize)); }
    { let weak = weak.clone(); ui.on_task_remove(move |i| todo::remove(&weak, i.max(0) as usize)); }

    // ---------- deck, tools, misc ----------
    ui.on_deck_action(tools::deck_action);
    {
        let weak = weak.clone();
        ui.on_deck_slot(move |i| {
            if !tools::launch_slot(i) {
                let msg = tools::fill_slot_from_clipboard(i);
                if let Some(ui) = weak.upgrade() {
                    ticker::toast(&ui, "Deck", "Quick launch", &msg);
                }
            }
        });
    }
    ui.on_deck_slot_remove(tools::clear_slot);
    ui.on_tool(|n| {
        if n == 3 {
            let _ = slint::quit_event_loop();
        } else {
            tools::home_tool(n);
        }
    });
    { let weak = weak.clone(); ui.on_optimize_memory(move || tools::optimize_memory(weak.clone())); }
    ui.on_bt_refresh(bluetooth::refresh);
    ui.on_weather_refresh(weather::refresh);
    {
        let companion = companion.clone();
        let weak = weak.clone();
        ui.on_poked(move || {
            let mood = companion.borrow_mut().poke(Instant::now());
            if let Some(ui) = weak.upgrade() {
                ui.set_mood(mood.as_int());
            }
            audio::play_sfx(mood.sfx());
        });
    }

    // ---------- background workers ----------
    media::spawn_poller(weak.clone());
    clipboard::spawn_watcher(weak.clone());
    webhook::spawn(weak.clone());
    weather::spawn(weak.clone());
    specs::spawn(weak.clone());
    sites::init(weak.clone());
    quick::spawn(weak.clone());
    notifications::spawn(weak.clone());
    bluetooth::spawn(weak.clone());

    // ---------- settings watcher (every 0.7 s) ----------
    let last_sig = Rc::new(RefCell::new(config.borrow().dump()));
    let last_capture = Rc::new(Cell::new(false));
    let slow_timer = Timer::default();
    {
        let weak = weak.clone();
        let config = config.clone();
        let hwnd_cell = hwnd_cell.clone();
        slow_timer.start(TimerMode::Repeated, Duration::from_millis(700), move || {
            let Some(ui) = weak.upgrade() else { return };
            let fresh = Config::load();
            let sig = fresh.dump();
            if *last_sig.borrow() != sig {
                *last_sig.borrow_mut() = sig;
                if !fresh.on("island_enabled") {
                    let _ = slint::quit_event_loop();
                    return;
                }
                look::apply(&ui, &fresh);
                *config.borrow_mut() = fresh;
            }
            let hide = ui.get_hide_capture();
            if hide != last_capture.get() {
                if let Some(h) = hwnd_cell.get() {
                    win32::window::set_capture_hidden(h, hide);
                    last_capture.set(hide);
                }
            }
        });
    }

    // ---------- main loop (every 16 ms) ----------
    let origin_x: Cell<i32> = Cell::new(i32::MIN);
    let last_region: Cell<(i32, i32, i32, i32)> = Cell::new((-1, -1, -1, -1));
    let cal_was_visible: Cell<bool> = Cell::new(false);

    let app_active: Cell<bool> = Cell::new(false);
    let last_fg_check: Cell<Option<Instant>> = Cell::new(None);
    let revealed: Cell<bool> = Cell::new(false);
    let edge_since: Cell<Option<Instant>> = Cell::new(None);
    let last_near: Cell<Instant> = Cell::new(Instant::now());
    let inside_since: Cell<Option<Instant>> = Cell::new(None);
    let collapsed_since: Cell<Option<Instant>> = Cell::new(None);

    let hover = RefCell::new(HoverTracker::new());
    let eyes = RefCell::new(Eyes::new());
    let tick_weak = weak.clone();
    let tick_cfg = config.clone();
    let tick_hwnd = hwnd_cell.clone();

    let tick_timer = Timer::default();
    tick_timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let Some(ui) = tick_weak.upgrade() else { return };
        let window = ui.window();
        let size = window.size();
        if size.width == 0 {
            return;
        }
        let scale = window.scale_factor();
        let now = Instant::now();

        let hwnd = match tick_hwnd.get() {
            Some(h) => h,
            None => {
                let Some(h) = win32::window::hwnd_of(window) else { return };
                tick_hwnd.set(Some(h));
                h
            }
        };
        win32::window::sync_styles(hwnd);

        // where the window sits on the screen
        let wx = win32::position::window_x(
            size.width as i32,
            ui.get_dock(),
            (ui.get_edge_margin_px() * scale) as i32,
        );
        if wx != origin_x.get() {
            win32::window::place(hwnd, wx, 0);
            origin_x.set(wx);
        }

        let Some((cx, cy)) = win32::cursor::position() else { return };
        let rel_x = (cx - origin_x.get()) as f32 / scale;
        let rel_y = cy as f32 / scale;

        let win_w = size.width as f32 / scale;
        let island_x = ui.get_island_x();
        let island_w = ui.get_island_w();
        let island_h = ui.get_island_h();
        let island_y = ui.get_island_y();
        let tucked = ui.get_tucked();
        let was_expanded = ui.get_expanded();
        let pinned = ui.get_pinned();
        let typing = ui.get_typing();
        let tuck_mode = ui.get_tuck_mode();

        let pad = if was_expanded { 8.0 } else { 4.0 };
        let near = rel_x >= island_x - pad
            && rel_x <= island_x + island_w + pad
            && rel_y >= -2.0
            && rel_y <= island_y + island_h + pad;

        // is another app in front? (only checked when it matters)
        if tuck_mode == 1 {
            if last_fg_check.get().map_or(true, |t| now.duration_since(t) >= Duration::from_millis(300)) {
                app_active.set(win32::foreground::other_app_in_use());
                last_fg_check.set(Some(now));
            }
        } else {
            app_active.set(false);
        }
        let covered = tuck_mode == 2 || (tuck_mode == 1 && app_active.get());

        // resting on the top edge for 2 s brings the tucked bar back
        let centre = island_x + island_w / 2.0;
        let at_edge = rel_y <= 2.0 && (rel_x - centre).abs() <= 84.0;
        if tucked && at_edge {
            let since = edge_since.get().unwrap_or(now);
            edge_since.set(Some(since));
            if now.duration_since(since) >= Duration::from_secs(2) {
                revealed.set(true);
                edge_since.set(None);
                last_near.set(now);
            }
        } else {
            edge_since.set(None);
        }

        if near || was_expanded {
            last_near.set(now);
        }
        if !covered {
            revealed.set(false);
        } else if revealed.get() && now.duration_since(last_near.get()) >= Duration::from_secs(3) {
            revealed.set(false);
        }

        let want_hidden =
            covered && !revealed.get() && !was_expanded && !ui.get_spec_pill() && !typing && !pinned;
        if want_hidden != ui.get_hidden() {
            ui.set_hidden(want_hidden);
        }

        // after the close button, stay closed until the cursor leaves
        if !near {
            suppress.set(false);
        }

        // open after the cursor rests on it (optionally with a delay)
        let want_inside = near && !tucked && !suppress.get();
        let delay = Duration::from_millis(ui.get_open_delay_ms().max(0.0) as u64);
        let inside_ok = if want_inside {
            let since = inside_since.get().unwrap_or(now);
            inside_since.set(Some(since));
            was_expanded || now.duration_since(since) >= delay
        } else {
            inside_since.set(None);
            false
        };
        let inside = inside_ok || pinned || typing;
        let expanded = hover.borrow_mut().update(inside, now);
        if expanded != was_expanded {
            ui.set_expanded(expanded);
        }
        if !expanded && ui.get_source_menu() {
            ui.set_source_menu(false);
        }

        // back to the first tab after being closed for a while
        if expanded {
            collapsed_since.set(None);
        } else {
            let since = collapsed_since.get().unwrap_or(now);
            collapsed_since.set(Some(since));
            if now.duration_since(since) >= Duration::from_secs(10) {
                let page = look::default_page(&tick_cfg.borrow());
                if ui.get_page() != page && ui.get_tab_ids().iter().any(|p| p == page) {
                    ui.set_page(page);
                }
                collapsed_since.set(Some(now));
            }
        }

        // only the island itself receives the mouse (the rest is click-through)
        let rpad = if tucked { 0.0 } else { app_state::REGION_PAD };
        let rl = (((island_x - rpad) * scale).floor() as i32).clamp(0, size.width as i32);
        let rr = (((island_x + island_w + rpad) * scale).ceil() as i32).clamp(0, size.width as i32);
        let rb = (((island_y + island_h + rpad).max(1.0) * scale).ceil() as i32).clamp(0, size.height as i32);
        let region = (rl, 0, rr, rb);
        if region != last_region.get() {
            win32::window::set_region(hwnd, rl, 0, rr, rb);
            last_region.set(region);
        }
        let _ = win_w;

        // the little dude follows the cursor
        let mx = ui.get_mascot_cx();
        let my = ui.get_mascot_cy();
        if let Some((lx, ly)) = eyes.borrow_mut().update(rel_x - mx, rel_y - my) {
            ui.set_look_x(lx);
            ui.set_look_y(ly);
        }
        let mood = companion.borrow_mut().tick(now);
        ui.set_mood(mood.as_int());

        // only measure things for the page that is open
        let page = ui.get_page();
        quick::set_wanted(expanded && (page == 2 || page == 3));
        bluetooth::set_wanted(expanded && (page == 0 || page == 3));
        specs::set_wanted((expanded && (page == 0 || page == 7)) || ui.get_spec_pill());

        let cal_visible = expanded && page == 10;
        if cal_visible && !cal_was_visible.get() {
            calendar::show_today(&ui);
        }
        cal_was_visible.set(cal_visible);

        let viz_wanted = expanded && ui.get_playing() && (page == 0 || page == 1) && ui.get_viz_on() && !ui.get_source_menu();
        viz.set_enabled(viz_wanted);
        sync_bars(&viz_model, &viz.levels());

        if expanded && page == 8 && ui.get_notif_unread() > 0 {
            ui.set_notif_unread(0);
        }
    });

    ui.run()
}
