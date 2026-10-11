//! Windows and macOS, through the `enigo` crate.

use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use super::Backend;

pub struct Native {
    enigo: Option<Enigo>,
}

impl Native {
    pub fn new() -> Self {
        Self { enigo: None }
    }
}

impl Backend for Native {
    fn name(&self) -> &'static str {
        if cfg!(windows) { "Windows" } else { "macOS" }
    }

    fn begin(&mut self) -> Result<(), String> {
        if self.enigo.is_none() {
            // On macOS this prompts for the Accessibility permission the first time.
            let enigo = Enigo::new(&Settings::default())
                .map_err(|e| format!("Cannot send keystrokes: {e}"))?;
            self.enigo = Some(enigo);
        }
        Ok(())
    }

    fn type_char(&mut self, c: char, hold: Duration) -> Result<(), String> {
        let enigo = self.enigo.as_mut().ok_or("not connected")?;
        let result = match c {
            '\n' | '\t' => {
                let key = if c == '\n' { Key::Return } else { Key::Tab };
                enigo.key(key, Direction::Press).and_then(|()| {
                    std::thread::sleep(hold);
                    enigo.key(key, Direction::Release)
                })
            }
            _ => enigo.text(c.encode_utf8(&mut [0; 4])),
        };
        result.map_err(|e| format!("Could not type {c:?}: {e}"))
    }

    fn escape_pressed(&mut self) -> bool {
        escape_is_down()
    }

    fn end(&mut self) {}
}

// Neither system lets an ordinary app take a key away from the focused window,
// so Escape is only observed: it stops the run but also reaches the target.

#[cfg(windows)]
fn escape_is_down() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(virtual_key: i32) -> i16;
    }
    const VK_ESCAPE: i32 = 0x1b;
    // The top bit says the key is down right now.
    unsafe { GetAsyncKeyState(VK_ESCAPE) < 0 }
}

#[cfg(target_os = "macos")]
fn escape_is_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    }
    const HID_SYSTEM_STATE: i32 = 1; // physical keys only, not the ones we post
    const ESCAPE: u16 = 53;
    unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, ESCAPE) }
}
