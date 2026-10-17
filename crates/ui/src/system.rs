//! Windows settings Slint does not read itself: "Animation effects" and high contrast.

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

#[cfg(not(windows))]
pub fn animations_enabled() -> bool {
    true
}

#[cfg(not(windows))]
pub fn high_contrast() -> bool {
    false
}
