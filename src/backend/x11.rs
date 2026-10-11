//! X11, through the XTest extension.
//!
//! Characters are typed the way a person would: the key that carries the character
//! in the active layout is pressed, together with Shift or AltGr when the layout
//! needs them. Browsers therefore see ordinary key events with real key codes.

use std::collections::HashMap;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _, KeySymMap, KeyType};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ConnectionExt as _, GrabMode, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, ModMask, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use xkeysym::Keysym;

use super::Backend;

const SHIFT: u8 = 1 << 0;
const LOCK: u8 = 1 << 1;
const MOD3: u8 = 1 << 5;
const MOD5: u8 = 1 << 7;
/// Modifiers we are willing to hold to reach a character: Shift, and the ones
/// layouts use for their third and fifth levels (AltGr and friends).
const LEVEL_MODS: u8 = SHIFT | MOD3 | MOD5;
const MAX_SPARES: usize = 16;
/// Modifiers that may be down when the user hits Escape mid-run: the ones we hold
/// ourselves, plus Num Lock (normally Mod2).
const ESCAPE_MODS: u8 = LEVEL_MODS | LOCK | (1 << 4);

/// A key, plus the modifiers (as a core modifier mask) to hold while pressing it.
#[derive(Clone, Copy)]
struct Stroke {
    keycode: u8,
    mods: u8,
}

struct Session {
    conn: RustConnection,
    root: Window,
    strokes: HashMap<char, Stroke>,
    enter: Option<u8>,
    tab: Option<u8>,
    escape: Option<u8>,
    /// First key code bound to each of the eight core modifiers.
    mod_keys: [u8; 8],
    /// Unused key codes that can be bound temporarily to characters the layout lacks,
    /// each with the character it is bound to right now.
    spares: Vec<(u8, Option<char>)>,
    next_spare: usize,
}

pub struct X11 {
    session: Option<Session>,
}

impl X11 {
    pub fn new() -> Self {
        Self { session: None }
    }
}

fn err(context: &str, e: impl std::fmt::Display) -> String {
    format!("X11: {context}: {e}")
}

/// The group (layout) a key actually uses when `group` is the active one.
fn effective_group(group: u8, group_info: u8) -> usize {
    let count = group_info & 0x0f;
    if group < count {
        return group as usize;
    }
    match group_info & 0xc0 {
        0x40 => (count - 1) as usize, // clamp
        0x80 => {
            // redirect
            let target = (group_info >> 4) & 0x03;
            if target < count { target as usize } else { 0 }
        }
        _ => (group % count) as usize, // wrap
    }
}

/// The smallest set of modifiers that selects `level` of a key of type `key_type`.
fn mods_for_level(key_type: &KeyType, level: usize) -> Option<u8> {
    if level == 0 {
        return Some(0);
    }
    key_type
        .map
        .iter()
        .filter(|entry| entry.active && entry.level as usize == level)
        .map(|entry| u16::from(entry.mods_mask) as u8)
        .filter(|mods| mods & !LEVEL_MODS == 0)
        .min_by_key(|mods| mods.count_ones())
}

impl Session {
    fn open() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| err("cannot connect", e))?;
        let root = conn.setup().roots[screen].root;
        let reply = |e: x11rb::errors::ReplyError| err("request failed", e);
        let send = |e: x11rb::errors::ConnectionError| err("connection lost", e);

        conn.xtest_get_version(2, 2)
            .map_err(send)?
            .reply()
            .map_err(|_| "This X server lacks the XTest extension.".to_string())?;
        conn.xkb_use_extension(1, 0)
            .map_err(send)?
            .reply()
            .map_err(|_| "This X server lacks the XKB extension.".to_string())?;

        let core = u16::from(xkb::ID::USE_CORE_KBD);
        let state = conn.xkb_get_state(core).map_err(send)?.reply().map_err(reply)?;
        if u16::from(state.locked_mods) as u8 & LOCK != 0 {
            return Err("Caps Lock is on. Turn it off and send again.".into());
        }
        let group = u8::from(state.group);

        let map = conn
            .xkb_get_map(
                core,
                xkb::MapPart::KEY_TYPES | xkb::MapPart::KEY_SYMS,
                Default::default(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                Default::default(),
                0,
                0,
                0,
                0,
                0,
                0,
            )
            .map_err(send)?
            .reply()
            .map_err(reply)?;
        let types = map.map.types_rtrn.unwrap_or_default();
        let keys = map.map.syms_rtrn.unwrap_or_default();

        let mut strokes: HashMap<char, Stroke> = HashMap::new();
        let (mut enter, mut tab, mut escape) = (None, None, None);
        let mut spares = Vec::new();
        for (index, key) in keys.iter().enumerate() {
            let Ok(keycode) = u8::try_from(map.first_key_sym as usize + index) else {
                break;
            };
            if key.group_info & 0x0f == 0 || key.width == 0 {
                spares.push((keycode, None));
                continue;
            }
            for (keysym, mods) in levels(key, group, &types) {
                match keysym {
                    0xff0d => enter = enter.or(Some(keycode)),
                    0xff09 => tab = tab.or(Some(keycode)),
                    0xff1b if mods == 0 => escape = escape.or(Some(keycode)),
                    // Function and keypad keys: never use those to produce text.
                    0xfd00..=0xffff => {}
                    _ => {
                        let Some(c) = Keysym::new(keysym).key_char() else {
                            continue;
                        };
                        let simpler = strokes
                            .get(&c)
                            .is_none_or(|old| mods.count_ones() < old.mods.count_ones());
                        if simpler {
                            strokes.insert(c, Stroke { keycode, mods });
                        }
                    }
                }
            }
        }

        let modifiers = conn.get_modifier_mapping().map_err(send)?.reply().map_err(reply)?;
        let per_modifier = modifiers.keycodes.len() / 8;
        let mut mod_keys = [0u8; 8];
        if per_modifier > 0 {
            for (slot, row) in mod_keys.iter_mut().zip(modifiers.keycodes.chunks(per_modifier)) {
                *slot = row.iter().copied().find(|k| *k != 0).unwrap_or(0);
            }
        }
        // A stroke is only usable if every modifier it needs has a key.
        strokes.retain(|_, stroke| (0..8).all(|bit| stroke.mods & (1 << bit) == 0 || mod_keys[bit] != 0));

        // Take Escape for ourselves while the run lasts, so it stops the typing
        // instead of reaching the target window. One grab per modifier combination,
        // because a catch-all grab fails if any other program uses e.g. Alt+Escape.
        // The grabs end with the connection.
        if let Some(escape) = escape {
            for mods in (0..=u8::MAX).filter(|mods| mods & !ESCAPE_MODS == 0) {
                conn.grab_key(
                    false,
                    root,
                    ModMask::from(u16::from(mods)),
                    escape,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .map_err(send)?
                .ignore_error();
            }
        }

        let session = Self {
            conn,
            root,
            strokes,
            enter,
            tab,
            escape,
            mod_keys,
            // The highest ones are the least likely to mean anything to other programs.
            spares: spares.into_iter().rev().take(MAX_SPARES).collect(),
            next_spare: 0,
        };
        // Make sure the grabs are in place before the countdown starts.
        session.sync()?;
        Ok(session)
    }

    fn fake(&self, event: u8, keycode: u8) -> Result<(), String> {
        self.conn
            .xtest_fake_input(event, keycode, x11rb::CURRENT_TIME, self.root, 0, 0, 0)
            .map(drop)
            .map_err(|e| err("connection lost", e))
    }

    fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| err("connection lost", e))
    }

    /// Waits until the server has processed everything sent so far. A server that
    /// sees the connection close may discard requests it has not read yet, and a
    /// discarded key release would leave the key held down.
    fn sync(&self) -> Result<(), String> {
        self.conn
            .get_input_focus()
            .map_err(|e| err("connection lost", e))?
            .reply()
            .map(drop)
            .map_err(|e| err("request failed", e))
    }

    /// Binds a single key code to `keysym` (0 unbinds it).
    fn bind(&self, keycode: u8, keysym: u32) -> Result<(), String> {
        // The keysym goes in both columns: a letter given only once would get
        // its lower-case form on the unshifted level.
        self.conn
            .change_keyboard_mapping(1, keycode, 2, &[keysym, keysym])
            .map_err(|e| err("connection lost", e))?;
        self.sync()
    }

    /// A key code that produces `c`, borrowed from the spare ones.
    fn borrow(&mut self, c: char, keysym: u32) -> Result<u8, String> {
        if let Some((keycode, _)) = self.spares.iter().find(|(_, bound)| *bound == Some(c)) {
            return Ok(*keycode);
        }
        if self.spares.is_empty() {
            return Err("no spare key code".into());
        }
        // Reuse them in turn once all are taken.
        let slot = self.next_spare % self.spares.len();
        self.next_spare += 1;
        let keycode = self.spares[slot].0;
        self.bind(keycode, keysym)?;
        self.spares[slot].1 = Some(c);
        // Give the receiving program a moment to pick up the new mapping.
        std::thread::sleep(Duration::from_millis(30));
        Ok(keycode)
    }

    fn release_spares(&mut self) {
        for (keycode, bound) in std::mem::take(&mut self.spares) {
            if bound.is_some() {
                let _ = self.bind(keycode, 0);
            }
        }
    }

    fn escape_pressed(&self) -> bool {
        let mut pressed = false;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if let Event::KeyPress(key) = event {
                pressed |= Some(key.detail) == self.escape;
            }
        }
        pressed
    }

    fn stroke_for(&mut self, c: char) -> Result<Stroke, String> {
        let known = match c {
            '\n' => self.enter.map(|keycode| Stroke { keycode, mods: 0 }),
            '\t' => self.tab.map(|keycode| Stroke { keycode, mods: 0 }),
            _ => self.strokes.get(&c).copied(),
        };
        if let Some(stroke) = known {
            return Ok(stroke);
        }
        // Not on the active layout: borrow an unused key code for it.
        let keysym = match c {
            '\n' => Keysym::Return,
            '\t' => Keysym::Tab,
            _ => Keysym::from_char(c),
        };
        if keysym == Keysym::NoSymbol {
            return Err(format!("There is no key for {c:?}."));
        }
        let keycode = self
            .borrow(c, keysym.raw())
            .map_err(|e| format!("{c:?} is not on the active keyboard layout ({e})."))?;
        Ok(Stroke { keycode, mods: 0 })
    }

    fn type_char(&mut self, c: char, hold: Duration) -> Result<(), String> {
        let stroke = self.stroke_for(c)?;
        let held: Vec<u8> = (0..8)
            .filter(|bit| stroke.mods & (1 << bit) != 0)
            .map(|bit| self.mod_keys[bit])
            .collect();
        for key in &held {
            self.fake(KEY_PRESS_EVENT, *key)?;
        }
        self.fake(KEY_PRESS_EVENT, stroke.keycode)?;
        self.flush()?;
        std::thread::sleep(hold);
        self.fake(KEY_RELEASE_EVENT, stroke.keycode)?;
        for key in held.iter().rev() {
            self.fake(KEY_RELEASE_EVENT, *key)?;
        }
        self.sync()
    }
}

/// Every `(keysym, modifiers)` a key offers in the active group.
fn levels<'a>(
    key: &'a KeySymMap,
    group: u8,
    types: &'a [KeyType],
) -> impl Iterator<Item = (u32, u8)> + 'a {
    let group = effective_group(group, key.group_info);
    let width = key.width as usize;
    let key_type = types.get(key.kt_index[group] as usize);
    (0..width).filter_map(move |level| {
        let keysym = *key.syms.get(group * width + level)?;
        let mods = mods_for_level(key_type?, level)?;
        (keysym != 0).then_some((keysym, mods))
    })
}

impl Backend for X11 {
    fn name(&self) -> &'static str {
        "X11"
    }

    fn begin(&mut self) -> Result<(), String> {
        // Reconnect for every run so the key map reflects the layout active right now.
        self.session = Some(Session::open()?);
        Ok(())
    }

    fn type_char(&mut self, c: char, hold: Duration) -> Result<(), String> {
        self.session.as_mut().ok_or("not connected")?.type_char(c, hold)
    }

    fn escape_pressed(&mut self) -> bool {
        self.session.as_ref().is_some_and(Session::escape_pressed)
    }

    fn on_layout(&self, c: char) -> bool {
        // Anything else goes through a borrowed key code. Native programs accept
        // that, but browsers report such a key press without its character.
        self.session
            .as_ref()
            .is_none_or(|session| session.strokes.contains_key(&c))
    }

    fn end(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.release_spares();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `SENDKEYS_TEST_TEXT` into whatever has the focus on `$DISPLAY`.
    /// Run it against a scratch X server, never a desktop in use:
    /// `cargo test -- --ignored types_into_focused_window`
    #[test]
    #[ignore = "sends real keystrokes to $DISPLAY"]
    fn types_into_focused_window() {
        let text = std::env::var("SENDKEYS_TEST_TEXT").expect("SENDKEYS_TEST_TEXT");
        let mut backend = X11::new();
        backend.begin().unwrap();
        for c in text.chars() {
            backend.type_char(c, Duration::from_millis(5)).unwrap();
        }
        backend.end();
    }
}
