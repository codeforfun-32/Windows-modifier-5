use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN};

/// Left edge for a window of the given physical width.
/// dock: 0 = centre, 1 = left, 2 = right. `margin_px` is the gap from the side edge when docked.
pub fn window_x(window_width_px: i32, dock: i32, margin_px: i32) -> i32 {
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let x = match dock {
        1 => margin_px,
        2 => screen_w - window_width_px - margin_px,
        _ => (screen_w - window_width_px) / 2,
    };
    x.max(0)
}
