//! Centres the preparation window on the primary screen's work area before it first shows (the C# window's
//! `WindowStartupLocation.CenterScreen`). Slint places a window it has not created yet at the position it is given.

use eg_ui::{PreparationWindow, Tokens};
use slint::ComponentHandle;

pub fn center(window: &PreparationWindow) {
    let tokens = window.global::<Tokens>();
    let (width, height) = (tokens.get_preparation_window_width(), tokens.get_preparation_window_height());
    if let Some((left, top, right, bottom, scale)) = work_area() {
        let x = left as f32 + ((right - left) as f32 - width * scale) / 2.0;
        let y = top as f32 + ((bottom - top) as f32 - height * scale) / 2.0;
        window
            .window()
            .set_position(slint::PhysicalPosition::new(x.max(left as f32) as i32, y.max(top as f32) as i32));
    }
}

/// The primary screen's work area in physical pixels, and the system's scale. The window's backend has made the process
/// DPI-aware by the time a window exists.
#[cfg(windows)]
fn work_area() -> Option<(i32, i32, i32, i32, f32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};

    let mut area = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: SPI_GETWORKAREA writes one RECT to the pointer it is given.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut area).cast(), 0) };
    // SAFETY: GetDpiForSystem takes no arguments and only reads the process's DPI awareness.
    let dpi = unsafe { GetDpiForSystem() };
    (ok != 0 && area.right > area.left && dpi > 0).then(|| (area.left, area.top, area.right, area.bottom, dpi as f32 / 96.0))
}

#[cfg(not(windows))]
fn work_area() -> Option<(i32, i32, i32, i32, f32)> {
    None
}
