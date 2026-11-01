//! Windows settings a toolkit may not read itself ("Animation effects" and high contrast), a dark title bar, and a message
//! box for when there is no window yet.

/// Whether Windows "Animation effects" is on. When the call fails, animation counts as on.
#[cfg(windows)]
pub fn animations_enabled() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

    let mut enabled: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes a single BOOL to the pointer it is given.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0) };
    ok == 0 || enabled != 0
}

/// Whether Windows reports a high-contrast (contrast theme) preference.
#[cfg(windows)]
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};

    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        dwFlags: 0,
        lpszDefaultScheme: std::ptr::null_mut(),
    };
    // SAFETY: SPI_GETHIGHCONTRAST fills a HIGHCONTRASTW whose cbSize is passed as uiParam.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, contrast.cbSize, (&raw mut contrast).cast(), 0) };
    ok != 0 && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// Asks DWM for a dark title bar and frame on the window `hwnd` names. A failure (Windows before 10 20H1, or a window that
/// is gone) leaves the default frame.
#[cfg(windows)]
pub fn use_dark_title_bar(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};

    let dark: i32 = 1;
    // SAFETY: DWM checks the handle and refuses one that names no window, so any value is sound.
    // DWMWA_USE_IMMERSIVE_DARK_MODE reads one BOOL of the size passed.
    unsafe {
        DwmSetWindowAttribute(hwnd as _, DWMWA_USE_IMMERSIVE_DARK_MODE as _, (&raw const dark).cast(), size_of::<i32>() as u32);
    }
}

/// Says something in a Windows message box, for a start that ends before it has a window to say it in.
#[cfg(windows)]
pub fn tell(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};

    let (title, text) = (crate::wide(title), crate::wide(text));
    // SAFETY: both strings are NUL-terminated and outlive this synchronous call, which has no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

#[cfg(not(windows))]
pub fn tell(_title: &str, _text: &str) {}

#[cfg(not(windows))]
pub fn animations_enabled() -> bool {
    true
}

#[cfg(not(windows))]
pub fn high_contrast() -> bool {
    false
}
