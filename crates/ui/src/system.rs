//! Windows settings Slint does not read itself ("Animation effects" and high contrast), and the dark title bar the
//! C# app asks for (App.axaml `RequestedThemeVariant="Dark"`).

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

/// Gives the window a dark title bar once its native window exists, as the C# app asks for one whatever the system theme
/// (App.axaml `RequestedThemeVariant="Dark"`). The winit window is created only after the event loop starts, so this
/// tries on a short timer and gives up after a second; a window with no native handle (headless, the captures' software
/// window) keeps whatever frame it has.
pub fn follow_dark_title_bar<C: slint::ComponentHandle + 'static>(component: &C) {
    const ATTEMPTS: u32 = 60;
    if use_dark_title_bar(component.window()) {
        return;
    }
    let weak = component.as_weak();
    let timer = std::rc::Rc::new(slint::Timer::default());
    let stop = std::rc::Rc::downgrade(&timer);
    let mut attempts = 0;
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(16), move || {
        attempts += 1;
        let done = weak.upgrade().is_none_or(|component| use_dark_title_bar(component.window()));
        if (done || attempts >= ATTEMPTS)
            && let Some(timer) = stop.upgrade()
        {
            timer.stop();
        }
    });
    // The timer stops itself; it is kept for the process's life, as `follow_system_settings` keeps its own.
    std::mem::forget(timer);
}

/// Asks DWM for a dark title bar and frame. False while `window` has no native handle yet.
#[cfg(windows)]
fn use_dark_title_bar(window: &slint::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};

    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return true;
    };
    let dark: i32 = 1;
    // SAFETY: the handle names this live window; DWMWA_USE_IMMERSIVE_DARK_MODE reads one BOOL of the size passed. A
    // failure (Windows before 10 20H1) leaves the default frame.
    unsafe {
        DwmSetWindowAttribute(
            handle.hwnd.get() as _,
            DWMWA_USE_IMMERSIVE_DARK_MODE as _,
            (&raw const dark).cast(),
            size_of::<i32>() as u32,
        );
    }
    true
}

/// Says something in a Windows message box, for a start that ends before it has a window to say it in.
#[cfg(windows)]
pub fn tell(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};

    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are NUL-terminated and outlive this synchronous call, which has no owner window.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

#[cfg(not(windows))]
pub fn tell(_title: &str, _text: &str) {}

#[cfg(not(windows))]
fn use_dark_title_bar(_window: &slint::Window) -> bool {
    true
}

#[cfg(not(windows))]
pub fn animations_enabled() -> bool {
    true
}

#[cfg(not(windows))]
pub fn high_contrast() -> bool {
    false
}
