//! Everything that talks to Windows directly.
use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetSystemMetrics,
    GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, PostMessageW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, GW_OWNER, SM_CXSCREEN, SM_CYSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SW_HIDE, SW_SHOWNA, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

const SHELL_CLASSES: [&str; 8] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
];

const WS_POPUP_BIT: isize = 0x8000_0000u32 as i32 as isize;
const WS_CHILD_BIT: isize = 0x4000_0000;

// ───────── our own window ─────────

pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// No taskbar button, and not part of Alt+Tab.
pub fn sync_styles(hwnd: HWND) {
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = (current | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        if wanted != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn set_visible(hwnd: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

// ───────── screen ─────────

pub fn screen() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

fn virtual_origin() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) }
}

// ───────── the desktop layer behind the icons ─────────

/// The WorkerW window that holds the desktop icons (older Windows layout).
fn icons_workerw() -> Option<HWND> {
    unsafe {
        let mut cur = GetWindow(GetDesktopWindow(), GW_CHILD).ok();
        let mut guard = 0;
        while let Some(h) = cur {
            guard += 1;
            if guard > 4000 || h.0.is_null() {
                break;
            }
            cur = GetWindow(h, GW_HWNDNEXT).ok();
            let mut cls = [0u16; 64];
            let n = GetClassNameW(h, &mut cls);
            if n > 0 && String::from_utf16_lossy(&cls[..n as usize]) == "WorkerW" {
                if FindWindowExW(Some(h), None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok() {
                    return Some(h);
                }
            }
        }
        None
    }
}

fn make_child(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let wanted = (style & !WS_POPUP_BIT) | WS_CHILD_BIT;
        if wanted != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, wanted);
        }
    }
}

/// Puts the wallpaper window into the desktop layer: behind the icons, behind every app.
/// Returns true when it found a place for it.
pub fn embed(hwnd: HWND, width: i32, height: i32) -> bool {
    unsafe {
        let Ok(progman) = FindWindowW(w!("Progman"), PCWSTR::null()) else {
            return false;
        };
        // asks Explorer to create the wallpaper layer
        let _ = PostMessageW(Some(progman), 0x052C, WPARAM(0xD), LPARAM(1));

        let (vx, vy) = virtual_origin();

        // Windows 10 and Windows 11 up to 23H2: an empty WorkerW sits right behind the one with the icons
        if let Some(icons) = icons_workerw() {
            if let Ok(host) = FindWindowExW(None, Some(icons), w!("WorkerW"), PCWSTR::null()) {
                if !host.0.is_null() {
                    let _ = SetParent(hwnd, Some(host));
                    make_child(hwnd);
                    let _ = SetWindowPos(hwnd, None, -vx, -vy, width, height, SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED);
                    return true;
                }
            }
        }

        // Windows 11 24H2 and newer: the wallpaper layer is a child of Progman
        if let Ok(layer) = FindWindowExW(Some(progman), None, w!("WorkerW"), PCWSTR::null()) {
            if !layer.0.is_null() {
                let _ = SetParent(hwnd, Some(progman));
                make_child(hwnd);
                let _ = SetWindowPos(hwnd, Some(layer), -vx, -vy, width, height, SWP_NOACTIVATE | SWP_FRAMECHANGED);
                return true;
            }
        }
        false
    }
}

/// Moves / resizes the wallpaper after it is attached (for example when the screen size changes).
pub fn fit_to_screen(hwnd: HWND, width: i32, height: i32) {
    let (vx, vy) = virtual_origin();
    unsafe {
        let _ = SetWindowPos(hwnd, None, -vx, -vy, width, height, SWP_NOACTIVATE | SWP_NOZORDER);
    }
}

// ───────── apps in front ─────────

pub struct AppWin {
    pub rect: (i32, i32, i32, i32),
    pub exe: String,
}

fn is_shell_or_me(h: HWND) -> bool {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(h, Some(&mut pid as *mut u32));
        if pid == std::process::id() || pid == 0 {
            return true;
        }
        // our own programs (settings, taskbar, island) do not count as "an app is open"
        if let Some(p) = exe_of(pid) {
            let p = p.to_lowercase();
            if p.ends_with("settings-app.exe") || p.ends_with("taskbar-app.exe") || p.ends_with("dynamic-island.exe") {
                return true;
            }
        }
        let mut cls = [0u16; 128];
        let n = GetClassNameW(h, &mut cls);
        if n > 0 {
            let c = String::from_utf16_lossy(&cls[..n as usize]);
            if SHELL_CLASSES.contains(&c.as_str()) {
                return true;
            }
        }
        false
    }
}

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if ok { Some(String::from_utf16_lossy(&buf[..len as usize])) } else { None }
    }
}

fn file_name_lower(path: &str) -> String {
    path.rsplit(|c: char| c == '\\' || c == '/').next().unwrap_or(path).to_lowercase()
}

/// The app windows that are open and not minimized, with the program each one belongs to.
pub fn app_windows(want_names: bool) -> Vec<AppWin> {
    let mut out = Vec::new();
    unsafe {
        let mut cur = GetWindow(GetDesktopWindow(), GW_CHILD).ok();
        let mut guard = 0;
        while let Some(h) = cur {
            guard += 1;
            if guard > 4000 || h.0.is_null() {
                break;
            }
            cur = GetWindow(h, GW_HWNDNEXT).ok();

            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                continue;
            }
            if let Ok(owner) = GetWindow(h, GW_OWNER) {
                if !owner.0.is_null() {
                    continue;
                }
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
                continue;
            }
            if GetWindowTextLengthW(h) == 0 {
                continue;
            }
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
            if cloaked != 0 {
                continue;
            }
            if is_shell_or_me(h) {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(h, &mut r).is_err() {
                continue;
            }
            if r.right - r.left < 40 || r.bottom - r.top < 40 {
                continue;
            }
            let mut exe = String::new();
            if want_names {
                let mut pid: u32 = 0;
                GetWindowThreadProcessId(h, Some(&mut pid as *mut u32));
                if let Some(p) = exe_of(pid) {
                    exe = file_name_lower(&p);
                }
            }
            out.push(AppWin { rect: (r.left, r.top, r.right, r.bottom), exe });
        }
    }
    out
}

/// A fullscreen game or video in front (normal maximised windows keep their title bar).
pub fn fullscreen_app() -> bool {
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() || !IsWindowVisible(h).as_bool() {
            return false;
        }
        if is_shell_or_me(h) {
            return false;
        }
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        if style & 0x00C0_0000 != 0 {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            return false;
        }
        let (sw, sh) = screen();
        r.left <= 0 && r.top <= 0 && r.right >= sw && r.bottom >= sh
    }
}

// ───────── power ─────────

/// True when the PC runs on battery (not plugged in).
pub fn on_battery() -> bool {
    unsafe {
        let mut s = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut s).is_ok() {
            s.ACLineStatus == 0
        } else {
            false
        }
    }
}
