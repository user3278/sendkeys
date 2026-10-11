//! Platform back ends that turn characters into key presses in the focused window.

use std::time::Duration;

#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;

pub trait Backend {
    /// Short description for the status bar.
    fn name(&self) -> &'static str;

    /// Gets ready to type. May block while the desktop asks the user for permission.
    fn begin(&mut self) -> Result<(), String>;

    /// Presses and releases the key(s) producing `c`, holding them down for `hold`.
    /// `'\n'` is the Enter key and `'\t'` the Tab key.
    fn type_char(&mut self, c: char, hold: Duration) -> Result<(), String>;

    /// Whether `c` has a key on the active keyboard layout, as far as the back end
    /// can tell. Only meaningful after `begin`.
    fn on_layout(&self, _c: char) -> bool {
        true
    }

    /// Whether the user pressed Escape since `begin`, wherever the focus is.
    /// Back ends that cannot watch the keyboard globally always say no.
    fn escape_pressed(&mut self) -> bool {
        false
    }

    /// Called when a run finishes, whether it completed, was stopped or failed.
    fn end(&mut self);
}

/// Picks the back end for the current session.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn create() -> Box<dyn Backend> {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    // SENDKEYS_BACKEND=x11|portal overrides the detection.
    let wayland = match var("SENDKEYS_BACKEND").as_str() {
        "x11" => false,
        "portal" => true,
        _ => !var("WAYLAND_DISPLAY").is_empty() || var("XDG_SESSION_TYPE") == "wayland",
    };
    if wayland {
        Box::new(portal::Portal::new())
    } else {
        Box::new(x11::X11::new())
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn create() -> Box<dyn Backend> {
    Box::new(native::Native::new())
}
