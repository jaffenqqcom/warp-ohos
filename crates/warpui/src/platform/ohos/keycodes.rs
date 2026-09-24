//! Mapping from OHOS xcomponent key events to warpui keystrokes and text.
//!
//! `ohos-native-bindings-zed` no longer carries modifier state on
//! [`KeyEventData`], so it is tracked across events by [`Modifiers`] and passed
//! in by the input layer. Key names follow the same convention as the winit
//! back-end: letters are uppercased while shift is held, the shifted form of
//! digit and symbol keys is baked into the key name, and modifier keys
//! themselves produce no keystroke.

use std::collections::HashSet;

use openharmony_ability::xcomponent::{Action, KeyCode, KeyEventData};

use crate::keymap::Keystroke;

/// Shifted output of the main-keyboard digit row (US layout).
const SHIFTED_DIGITS: [char; 10] = ['!', '@', '#', '$', '%', '^', '&', '*', '(', ')'];

/// Modifier and lock state carried across key events.
///
/// Held keys are tracked by key code in a set rather than by a count per
/// modifier, because a held key repeats its down action; a count would rise on
/// every repeat and never return to zero.
#[derive(Debug, Default)]
pub struct Modifiers {
    held: HashSet<u32>,
    capslock: bool,
}

impl Modifiers {
    /// Folds one key event into the state.
    ///
    /// Returns `true` when the event was a modifier or lock key: those update
    /// the state and must not produce a keystroke or text of their own.
    pub fn update(&mut self, event: &KeyEventData) -> bool {
        let pressed = match event.action {
            Action::Down => true,
            Action::Up => false,
            Action::Unknown => {
                log::warn!(
                    "ohos::keycodes::Modifiers::update: dropping key event with unknown action, code={:?}",
                    event.code
                );
                return false;
            }
        };

        match event.code {
            KeyCode::CtrlLeft | KeyCode::CtrlRight => {}
            KeyCode::AltLeft | KeyCode::AltRight => {}
            KeyCode::ShiftLeft | KeyCode::ShiftRight => {}
            KeyCode::CapsLock => {
                if pressed {
                    self.capslock = !self.capslock;
                }
                log::debug!(
                    "ohos::keycodes::Modifiers::update: capslock={}",
                    self.capslock
                );
                return true;
            }
            _ => return false,
        }

        let code = event.code as u32;
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
        log::debug!(
            "ohos::keycodes::Modifiers::update: code={:?} pressed={pressed} -> ctrl={} alt={} shift={}",
            event.code,
            self.ctrl(),
            self.alt(),
            self.shift()
        );
        true
    }

    /// Forgets every held key, keeping the lock state.
    ///
    /// The key-up of a held modifier is not delivered when the window loses
    /// focus, so the state has to be reset on focus loss or the modifier stays
    /// stuck.
    pub fn clear_held(&mut self) {
        if !self.held.is_empty() {
            log::info!(
                "ohos::keycodes::Modifiers::clear_held: dropping {} held key(s)",
                self.held.len()
            );
        }
        self.held.clear();
    }

    pub fn ctrl(&self) -> bool {
        self.holds(KeyCode::CtrlLeft) || self.holds(KeyCode::CtrlRight)
    }

    pub fn alt(&self) -> bool {
        self.holds(KeyCode::AltLeft) || self.holds(KeyCode::AltRight)
    }

    pub fn shift(&self) -> bool {
        self.holds(KeyCode::ShiftLeft) || self.holds(KeyCode::ShiftRight)
    }

    pub fn capslock(&self) -> bool {
        self.capslock
    }

    fn holds(&self, code: KeyCode) -> bool {
        self.held.contains(&(code as u32))
    }
}

/// Converts an OHOS key event into the corresponding warpui keystroke, or `None`
/// when the event maps to none (modifier keys, unknown codes).
pub fn key_event_to_keystroke(event: &KeyEventData, modifiers: &Modifiers) -> Option<Keystroke> {
    let ctrl = modifiers.ctrl();
    let alt = modifiers.alt();
    let shift = modifiers.shift();
    let key = key_name(event.code, shift)?;
    log::info!(
        "key_event_to_keystroke: code={:?} key={key} ctrl={ctrl} alt={alt} shift={shift}",
        event.code
    );
    // OHOS reports no Meta/super key state, so `cmd` and `meta` stay false and
    // `cmdorctrl` bindings resolve to ctrl.
    Some(Keystroke {
        ctrl,
        alt,
        shift,
        cmd: false,
        meta: false,
        key,
    })
}

/// The key that would have been produced without any modifier, including
/// shift, or `None` when the key produces no name.
///
/// The kitty keyboard protocol uses it to identify a key independently of the
/// shifted form the active layout produced, e.g. shift+`1` reports `!` as the
/// key but `1` here.
pub fn key_without_modifiers(event: &KeyEventData) -> Option<String> {
    key_name(event.code, false)
}

/// The text this key event inserts, honouring shift, caps lock and ctrl, or
/// `None` when it produces no text at all.
///
/// Letters follow the desktop convention where shift inverts caps lock; ctrl
/// yields the matching control character.
pub fn key_event_to_chars(event: &KeyEventData, modifiers: &Modifiers) -> Option<String> {
    let ctrl = modifiers.ctrl();
    let shift = modifiers.shift();
    let base = text_base(event.code, shift, modifiers.capslock())?;
    let text = if ctrl { control_character(base)? } else { base };
    log::debug!(
        "key_event_to_chars: code={:?} text={text:?} ctrl={ctrl} shift={shift}",
        event.code
    );
    Some(text.to_string())
}

/// Maps a printable ASCII character to the control character ctrl produces with
/// it: `@` and the lowercase letters give `\x00`..`\x1a`, and the punctuation
/// block `[`, `\`, `]`, `^`, `_` continues to `\x1b`..`\x1f`.
fn control_character(base: char) -> Option<char> {
    match base {
        '@' => Some('\u{0}'),
        'a'..='z' => Some(char::from(base as u8 - b'a' + 1)),
        '['..='_' => Some(char::from(base as u8 - b'[' + 27)),
        _ => None,
    }
}

/// The unmodified text a key produces, before ctrl is folded in.
fn text_base(code: KeyCode, shift: bool, capslock: bool) -> Option<char> {
    if let Some(index) = letter_index(code) {
        let upper = shift ^ capslock;
        return Some(if upper {
            char::from(b'A' + index)
        } else {
            char::from(b'a' + index)
        });
    }
    if let Some(index) = digit_index(code) {
        return Some(if shift {
            SHIFTED_DIGITS[index]
        } else {
            char::from(b'0' + index as u8)
        });
    }
    if let Some(index) = numpad_digit_index(code) {
        return Some(char::from(b'0' + index as u8));
    }

    match code {
        KeyCode::Space => Some(' '),
        KeyCode::Comma => Some(symbol(',', '<', shift)),
        KeyCode::Period => Some(symbol('.', '>', shift)),
        KeyCode::Slash => Some(symbol('/', '?', shift)),
        KeyCode::Semicolon => Some(symbol(';', ':', shift)),
        KeyCode::Apostrophe => Some(symbol('\'', '"', shift)),
        KeyCode::LeftBracket => Some(symbol('[', '{', shift)),
        KeyCode::RightBracket => Some(symbol(']', '}', shift)),
        KeyCode::Backslash => Some(symbol('\\', '|', shift)),
        KeyCode::Minus => Some(symbol('-', '_', shift)),
        KeyCode::Equals => Some(symbol('=', '+', shift)),
        KeyCode::Grave => Some(symbol('`', '~', shift)),
        KeyCode::At => Some('@'),
        KeyCode::Plus => Some('+'),
        KeyCode::Star => Some('*'),
        KeyCode::Pound => Some('#'),
        _ => None,
    }
}

/// Key name in the convention expected by [`crate::keymap::Keystroke`], or
/// `None` for key codes that do not produce a keystroke.
fn key_name(code: KeyCode, shift: bool) -> Option<String> {
    if let Some(index) = letter_index(code) {
        return Some(
            (if shift {
                char::from(b'A' + index)
            } else {
                char::from(b'a' + index)
            })
            .to_string(),
        );
    }
    if let Some(index) = digit_index(code) {
        return Some(
            (if shift {
                SHIFTED_DIGITS[index]
            } else {
                char::from(b'0' + index as u8)
            })
            .to_string(),
        );
    }
    if let Some(index) = numpad_digit_index(code) {
        return Some(char::from(b'0' + index as u8).to_string());
    }
    if let Some(number) = f_key_number(code) {
        return Some(format!("f{number}"));
    }

    let name = match code {
        KeyCode::Enter | KeyCode::NumpadEnter => "enter",
        KeyCode::Tab => "tab",
        KeyCode::Space => " ",
        KeyCode::Del => "backspace",
        KeyCode::ForwardDel => "delete",
        KeyCode::Escape => "escape",
        KeyCode::MoveHome => "home",
        KeyCode::MoveEnd => "end",
        KeyCode::Insert => "insert",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::DpadUp => "up",
        KeyCode::DpadDown => "down",
        KeyCode::DpadLeft => "left",
        KeyCode::DpadRight => "right",
        KeyCode::Comma => return Some(symbol(',', '<', shift).to_string()),
        KeyCode::Period => return Some(symbol('.', '>', shift).to_string()),
        KeyCode::Slash => return Some(symbol('/', '?', shift).to_string()),
        KeyCode::Semicolon => return Some(symbol(';', ':', shift).to_string()),
        KeyCode::Apostrophe => return Some(symbol('\'', '"', shift).to_string()),
        KeyCode::LeftBracket => return Some(symbol('[', '{', shift).to_string()),
        KeyCode::RightBracket => return Some(symbol(']', '}', shift).to_string()),
        KeyCode::Backslash => return Some(symbol('\\', '|', shift).to_string()),
        KeyCode::Minus => return Some(symbol('-', '_', shift).to_string()),
        KeyCode::Equals => return Some(symbol('=', '+', shift).to_string()),
        KeyCode::Grave => return Some(symbol('`', '~', shift).to_string()),
        KeyCode::At => "@",
        KeyCode::Plus => "+",
        KeyCode::Star => "*",
        KeyCode::Pound => "#",
        KeyCode::NumpadDivide => "/",
        KeyCode::NumpadMultiply => "*",
        KeyCode::NumpadSubtract => "-",
        KeyCode::NumpadAdd => "+",
        KeyCode::NumpadDot => ".",
        _ => return None,
    };
    Some(name.to_string())
}

/// Index of a letter key within the `A..=Z` declaration range. Relies on
/// fieldless enum variants being contiguous in declaration order.
fn letter_index(code: KeyCode) -> Option<u8> {
    let raw = code as u32;
    let start = KeyCode::A as u32;
    let end = KeyCode::Z as u32;
    (start..=end).contains(&raw).then(|| (raw - start) as u8)
}

/// Index of a main-keyboard digit within the `Key0..=Key9` declaration range.
fn digit_index(code: KeyCode) -> Option<usize> {
    let raw = code as u32;
    let start = KeyCode::Key0 as u32;
    let end = KeyCode::Key9 as u32;
    (start..=end).contains(&raw).then(|| (raw - start) as usize)
}

/// Index of a numpad digit within the `Numpad0..=Numpad9` declaration range.
fn numpad_digit_index(code: KeyCode) -> Option<usize> {
    let raw = code as u32;
    let start = KeyCode::Numpad0 as u32;
    let end = KeyCode::Numpad9 as u32;
    (start..=end).contains(&raw).then(|| (raw - start) as usize)
}

/// Function-key number (1..=24). `F1`..`F12` and `F13`..`F24` are each
/// contiguous, but variants such as `NumLock` are declared between them, so the
/// range is checked in two segments rather than one `F1..=F24` span.
fn f_key_number(code: KeyCode) -> Option<u8> {
    let raw = code as u32;
    let f1 = KeyCode::F1 as u32;
    let f12 = KeyCode::F12 as u32;
    let f13 = KeyCode::F13 as u32;
    let f24 = KeyCode::F24 as u32;
    if (f1..=f12).contains(&raw) {
        Some((raw - f1) as u8 + 1)
    } else if (f13..=f24).contains(&raw) {
        Some((raw - f13) as u8 + 13)
    } else {
        None
    }
}

/// Chooses between the unshifted and shifted form of a symbol key.
fn symbol(base: char, shifted: char, shift: bool) -> char {
    if shift { shifted } else { base }
}
