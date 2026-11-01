//! Windows settings Slint does not read itself ("Animation effects" and high contrast), and the dark title bar.

pub use win_platform::system::{animations_enabled, high_contrast, tell};

/// Gives the window a dark title bar once its native window exists. The app's palette is always dark, so the title bar is
/// too. The winit window is created only after the event loop starts, so this
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

    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return true;
    };
    win_platform::system::use_dark_title_bar(handle.hwnd.get());
    true
}

#[cfg(not(windows))]
fn use_dark_title_bar(_window: &slint::Window) -> bool {
    true
}
