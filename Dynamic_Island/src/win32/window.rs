use std::sync::atomic::{AtomicBool, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// True while a text box in the island needs the keyboard (the window may take focus then).
static ALLOW_FOCUS: AtomicBool = AtomicBool::new(false);

/// Native window handle of the Slint window (only exists once the window is shown).
pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut core::ffi::c_void)),
        _ => None,
    }
}

/// Move the window without resizing or stealing focus.
pub fn place(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Keep the window out of Alt-Tab and make sure it never takes focus
/// (except while the user is typing into one of its text boxes).
pub fn sync_styles(hwnd: HWND) {
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let mut wanted = current | WS_EX_TOOLWINDOW.0 as isize;
        if ALLOW_FOCUS.load(Ordering::Relaxed) {
            wanted &= !(WS_EX_NOACTIVATE.0 as isize);
        } else {
            wanted |= WS_EX_NOACTIVATE.0 as isize;
        }
        if wanted != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

/// Let the island receive keyboard input (on) or go back to never taking focus (off).
pub fn set_text_focus(hwnd: HWND, on: bool) {
    ALLOW_FOCUS.store(on, Ordering::Relaxed);
    if on {
        sync_styles(hwnd);
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Hide the island from screenshots and screen sharing.
pub fn set_capture_hidden(hwnd: HWND, hidden: bool) {
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, if hidden { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}

/// Only this rectangle (physical px, window-relative) receives mouse input.
/// Everything outside it is click-through, so the island never blocks other apps.
pub fn set_region(hwnd: HWND, left: i32, top: i32, right: i32, bottom: i32) {
    unsafe {
        let region = CreateRectRgn(left, top, right, bottom);
        // after this call Windows owns the region
        let _ = SetWindowRgn(hwnd, Some(region), true);
    }
}
