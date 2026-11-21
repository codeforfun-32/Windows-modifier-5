//! Turns the settings (config.json) into island looks and behaviour.
use shared::Config;
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::{clipboard, DeckSlot, MainWindow, Theme};

/// Tab names, in the order of their numbers (the number is the page number in main.slint).
pub const TAB_NAMES: [&str; 12] = [
    "Home",
    "Music",
    "Quick settings",
    "Bluetooth",
    "Clipboard",
    "Focus",
    "Weather",
    "System",
    "Notifications",
    "Sites",
    "Calendar",
    "Deck",
];

pub fn tab_id(name: &str) -> Option<i32> {
    TAB_NAMES.iter().position(|t| *t == name).map(|i| i as i32)
}

fn argb(a: u8, r: u8, g: u8, b: u8) -> Color {
    Color::from_argb_u8(a, r, g, b)
}

fn hex(s: &str) -> (u8, u8, u8) {
    let v = u32::from_str_radix(s.trim().trim_start_matches('#'), 16).unwrap_or(0x7cf0c9);
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// (background r, g, b, alpha), is it a light theme?
fn palette(name: &str) -> ((u8, u8, u8, u8), bool) {
    match name {
        "Graphite" => ((0x1d, 0x20, 0x28, 0xf4), false),
        "Midnight" => ((0x0b, 0x10, 0x26, 0xf4), false),
        "Ocean" => ((0x0a, 0x22, 0x33, 0xf4), false),
        "Forest" => ((0x0c, 0x1f, 0x16, 0xf4), false),
        "Wine" => ((0x26, 0x0c, 0x16, 0xf4), false),
        "Sand" => ((0xe9, 0xdc, 0xc3, 0xf6), true),
        "Glass" => ((0x1b, 0x1c, 0x20, 0xb8), false),
        "Frosted" => ((0xdf, 0xe6, 0xf0, 0xc4), true),
        "Paper" => ((0xf4, 0xf1, 0xea, 0xf6), true),
        _ => ((0x06, 0x06, 0x08, 0xff), false),
    }
}

fn apply_theme(ui: &MainWindow, cfg: &Config) {
    let ((r, g, b, a), light) = palette(&cfg.get("surface"));
    let (ar, ag, ab) = hex(&cfg.get("accent"));
    let luma = 0.299 * ar as f32 + 0.587 * ag as f32 + 0.114 * ab as f32;
    let high = cfg.on("high_contrast");
    let t = ui.global::<Theme>();

    t.set_surface(argb(a, r, g, b));
    t.set_accent(argb(255, ar, ag, ab));
    if luma > 150.0 {
        t.set_on_accent(argb(255, 0x10, 0x12, 0x16));
    } else {
        t.set_on_accent(argb(255, 0xff, 0xff, 0xff));
    }
    t.set_sheen_top(argb(0, 255, 255, 255));
    t.set_danger(argb(255, 0xff, 0x6b, 0x6b));
    t.set_mascot(argb(255, 0xff, 0xd4, 0x79));
    t.set_mascot_ink(argb(255, 0x2b, 0x20, 0x14));

    if light {
        let ink = (0x1b, 0x1d, 0x22);
        t.set_card(argb(0x12, 0, 0, 0));
        t.set_card_hi(argb(0x20, 0, 0, 0));
        t.set_border(argb(if high { 0x90 } else { 0x2a }, 0, 0, 0));
        t.set_chip(argb(0x16, 0, 0, 0));
        t.set_chip_active(argb(0x2e, 0, 0, 0));
        t.set_track(argb(0x26, 0, 0, 0));
        t.set_text(argb(255, ink.0, ink.1, ink.2));
        t.set_text_dim(argb(if high { 0xe6 } else { 0xa8 }, ink.0, ink.1, ink.2));
        t.set_fill(argb(255, ink.0, ink.1, ink.2));
        t.set_on_fill(argb(255, 0xf7, 0xf7, 0xf9));
    } else {
        t.set_card(argb(0x10, 255, 255, 255));
        t.set_card_hi(argb(0x1e, 255, 255, 255));
        t.set_border(argb(if high { 0xa0 } else { 0x22 }, 255, 255, 255));
        t.set_chip(argb(0x16, 255, 255, 255));
        t.set_chip_active(argb(0x30, 255, 255, 255));
        t.set_track(argb(0x26, 255, 255, 255));
        t.set_text(argb(255, 0xf5, 0xf5, 0xf7));
        t.set_text_dim(argb(if high { 0xea } else { 0xa6 }, 0xf5, 0xf5, 0xf7));
        t.set_fill(argb(255, 0xf5, 0xf5, 0xf7));
        t.set_on_fill(argb(255, 0x0a, 0x0a, 0x0d));
    }
}

fn slot(cfg: &Config, n: usize) -> DeckSlot {
    let path = cfg.get(&format!("launch_{n}"));
    let path = path.trim().trim_matches('"').to_string();
    if path.is_empty() {
        return DeckSlot { name: "".into(), letter: "".into(), used: false, hue: 0 };
    }
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    DeckSlot {
        letter: crate::notifications::letter_of(&stem).into(),
        hue: crate::notifications::hue_of(&stem),
        name: stem.into(),
        used: true,
    }
}

/// Which tab opens first (page number).
pub fn default_page(cfg: &Config) -> i32 {
    tab_id(&cfg.get("open_tab")).unwrap_or(0)
}

/// Push every setting into the window.
pub fn apply(ui: &MainWindow, cfg: &Config) {
    apply_theme(ui, cfg);

    // shape of the island
    ui.set_shape(match cfg.get("shape").as_str() {
        "Notch" => 0,
        "Floating" => 2,
        _ => 1,
    });
    ui.set_top_gap(cfg.num("top_gap") as f32);
    ui.set_corner_style(match cfg.get("corners").as_str() {
        "Sharp" => 0,
        "Pill" => 2,
        _ => 1,
    });
    let (iw, ih) = match cfg.get("size").as_str() {
        "Small" => (160.0, 30.0),
        "Large" => (230.0, 42.0),
        _ => (190.0, 36.0),
    };
    ui.set_idle_w(iw);
    ui.set_idle_h(ih);
    let (ow, oh) = match cfg.get("open_size").as_str() {
        "Compact" => (720.0, 310.0),
        "Roomy" => (840.0, 364.0),
        _ => (780.0, 336.0),
    };
    ui.set_open_w(ow);
    ui.set_open_h(oh);

    // animation speed
    let base = match cfg.get("motion").as_str() {
        "Snappy" => 1.8,
        "Bouncy" => 0.85,
        _ => 1.0,
    };
    let mut speed = base * (cfg.num("anim_speed").clamp(50, 200) as f32 / 100.0);
    if cfg.on("reduce_motion") {
        speed *= 2.5;
    }
    ui.set_speed(speed.max(0.3));

    ui.set_dock(match cfg.get("position").as_str() {
        "Left" => 1,
        "Right" => 2,
        _ => 0,
    });
    ui.set_edge_margin_px(cfg.num("edge_margin") as f32);

    // what the idle island shows
    ui.set_idle_mode(match cfg.get("resting_show").as_str() {
        "Now playing" => 1,
        "Clock" => 2,
        "Nothing" => 3,
        _ => 0,
    });

    // music
    ui.set_show_art(cfg.on("np_artwork"));
    ui.set_show_progress(cfg.on("np_progress"));
    ui.set_music_tint(cfg.on("music_bg"));
    let viz = cfg.get("visualizer");
    ui.set_viz_on(viz != "Off");
    ui.set_viz_style(if viz == "Bars" { 0 } else { 1 });
    ui.set_progress_style(match cfg.get("progress_style").as_str() {
        "Thick" => 1,
        "Dot" => 2,
        _ => 0,
    });
    ui.set_live_timer(cfg.on("live_timer"));

    // pop-ups
    let mut secs = cfg.num("notif_time").clamp(2, 15) as f32;
    if cfg.on("longer_notices") {
        secs *= 2.0;
    }
    ui.set_toast_ms(secs * 1000.0);
    crate::notifications::set_mirror(cfg.on("mirror_notifs"));
    clipboard::configure(cfg.on("clip_history"), cfg.num("clip_count").clamp(5, 50) as usize);
    crate::calendar::set_24h(cfg.get("tb_clock_fmt") == "24 hour");

    // tuck-away behaviour: 0 off, 1 while another app is open, 2 always
    ui.set_tuck_mode(if cfg.on("hidden_until_hover") {
        2
    } else if cfg.on("autotuck") {
        1
    } else {
        0
    });
    ui.set_open_delay_ms(cfg.num("open_delay") as f32);
    ui.set_hide_capture(cfg.on("hide_capture"));

    // tabs in the chosen order
    let mut ids: Vec<i32> = Vec::new();
    for slot_no in 1..=12 {
        let name = cfg.get(&format!("tab_{slot_no}"));
        let Some(id) = tab_id(&name) else { continue };
        let hidden = (id == 8 && !cfg.on("notifs_tab"))
            || (id == 2 && !cfg.on("controls_tab"))
            || (id == 9 && !cfg.on("web_tab"));
        if hidden || ids.contains(&id) {
            continue;
        }
        ids.push(id);
    }
    if ids.is_empty() {
        ids.push(0);
    }
    if !ids.contains(&ui.get_page()) {
        ui.set_page(ids[0]);
    }
    ui.set_tab_ids(ModelRc::new(VecModel::from(ids)));

    // quick launch slots
    let slots: Vec<DeckSlot> = (1..=6).map(|n| slot(cfg, n)).collect();
    ui.set_deck_slots(ModelRc::new(VecModel::from(slots)));
}
