//! Small helpers behind the Deck and Home buttons: key presses, launching apps, memory clean-up.
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::Duration;

use slint::Weak;
use sysinfo::System;
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, KEYBD_EVENT_FLAGS};

use crate::MainWindow;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Press all keys together (in order), then release them (in reverse order).
pub fn press(keys: &[u8]) {
    unsafe {
        for k in keys {
            keybd_event(*k, 0, KEYBD_EVENT_FLAGS(0), 0);
        }
        for k in keys.iter().rev() {
            keybd_event(*k, 0, KEYEVENTF_KEYUP, 0);
        }
    }
}

/// Deck big buttons: 0 screenshot, 1 snip, 2 screen recording.
pub fn deck_action(index: i32) {
    // wait a moment so the click on the island is finished first
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        match index {
            0 => press(&[0x2C]),             // Print Screen
            1 => press(&[0x5B, 0x10, 0x53]), // Win + Shift + S
            _ => press(&[0x5B, 0x12, 0x52]), // Win + Alt + R
        }
    });
}

/// Home tools: 0 calculator, 1 snip, 2 task manager. (3 = quit is handled in main.)
pub fn home_tool(index: i32) {
    match index {
        0 => {
            let _ = Command::new("calc.exe").creation_flags(CREATE_NO_WINDOW).spawn();
        }
        1 => deck_action(1),
        2 => {
            let _ = Command::new("taskmgr.exe").spawn();
        }
        _ => {}
    }
}

fn slot_key(n: i32) -> String {
    format!("launch_{}", n + 1)
}

/// Open the app / file / folder saved in a deck slot. Returns false when the slot is empty.
pub fn launch_slot(index: i32) -> bool {
    let cfg = shared::Config::load();
    let path = cfg.get(&slot_key(index));
    let path = path.trim().trim_matches('"').to_string();
    if path.is_empty() {
        return false;
    }
    let _ = Command::new("explorer.exe").arg(&path).spawn();
    true
}

/// Put the path from the clipboard into this slot. Returns the message to show.
pub fn fill_slot_from_clipboard(index: i32) -> String {
    let text = arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
        .unwrap_or_default();
    let path = text.trim().trim_matches('"').to_string();
    if path.is_empty() || !std::path::Path::new(&path).exists() {
        return "Copy an app or file path first (Shift + right-click > Copy as path)".to_string();
    }
    let mut cfg = shared::Config::load();
    cfg.set(&slot_key(index), &path);
    cfg.save();
    "Saved to the deck".to_string()
}

pub fn clear_slot(index: i32) {
    let mut cfg = shared::Config::load();
    cfg.set(&slot_key(index), "");
    cfg.save();
}

/// Ask Windows to trim the memory of every program, then tell how much was freed.
pub fn optimize_memory(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        let mut sys = System::new();
        sys.refresh_memory();
        let before = sys.used_memory();

        let script = "Get-Process | ForEach-Object { try { $_.MinWorkingSet = $_.MinWorkingSet } catch {} }; \
                      $sig='[DllImport(\"psapi.dll\")] public static extern int EmptyWorkingSet(IntPtr h);'; \
                      $t=Add-Type -MemberDefinition $sig -Name W -Namespace N -PassThru; \
                      Get-Process | ForEach-Object { try { [void]$t::EmptyWorkingSet($_.Handle) } catch {} }";
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        thread::sleep(Duration::from_millis(600));

        sys.refresh_memory();
        let after = sys.used_memory();
        let freed_mb = before.saturating_sub(after) / (1024 * 1024);
        let body = format!("Freed about {freed_mb} MB");
        let _ = ui.upgrade_in_event_loop(move |ui| {
            crate::ticker::toast(&ui, "System", "Memory optimized", &body);
        });
    });
}
