//! Mapping between warpui keystrokes and the OHOS hotkey key codes.
//!
//! A global shortcut is handed to the OHOS input service as a list of modifier
//! keys plus one final key, both named by the `KEYCODE_*` values of
//! `multimodalinput/oh_key_code.h` (API 14+, no permission gate). The key names
//! this module reads and produces follow the same convention as
//! [`super::keycodes`]: letters are uppercased while shift is held, and the
//! shifted form of digit and symbol keys is baked into the name. A shortcut the
//! settings editor recorded therefore round-trips through registration and back
//! to the same [`Keystroke`], which is what the global-shortcut table matches
//! on.

use openharmony_ability::MAX_PRE_KEYS;

use crate::keymap::Keystroke;

/// Key codes of `multimodalinput/oh_key_code.h`.
mod keycode {
    pub(super) const KEY_0: i32 = 2000;
    pub(super) const DPAD_UP: i32 = 2012;
    pub(super) const DPAD_DOWN: i32 = 2013;
    pub(super) const DPAD_LEFT: i32 = 2014;
    pub(super) const DPAD_RIGHT: i32 = 2015;
    pub(super) const A: i32 = 2017;
    pub(super) const Z: i32 = 2042;
    pub(super) const COMMA: i32 = 2043;
    pub(super) const PERIOD: i32 = 2044;
    pub(super) const ALT_LEFT: i32 = 2045;
    pub(super) const ALT_RIGHT: i32 = 2046;
    pub(super) const SHIFT_LEFT: i32 = 2047;
    pub(super) const SHIFT_RIGHT: i32 = 2048;
    pub(super) const TAB: i32 = 2049;
    pub(super) const SPACE: i32 = 2050;
    pub(super) const ENTER: i32 = 2054;
    pub(super) const DEL: i32 = 2055;
    pub(super) const GRAVE: i32 = 2056;
    pub(super) const MINUS: i32 = 2057;
    pub(super) const EQUALS: i32 = 2058;
    pub(super) const LEFT_BRACKET: i32 = 2059;
    pub(super) const RIGHT_BRACKET: i32 = 2060;
    pub(super) const BACKSLASH: i32 = 2061;
    pub(super) const SEMICOLON: i32 = 2062;
    pub(super) const APOSTROPHE: i32 = 2063;
    pub(super) const SLASH: i32 = 2064;
    pub(super) const PAGE_UP: i32 = 2068;
    pub(super) const PAGE_DOWN: i32 = 2069;
    pub(super) const ESCAPE: i32 = 2070;
    pub(super) const FORWARD_DEL: i32 = 2071;
    pub(super) const CTRL_LEFT: i32 = 2072;
    pub(super) const CTRL_RIGHT: i32 = 2073;
    pub(super) const META_LEFT: i32 = 2076;
    pub(super) const META_RIGHT: i32 = 2077;
    pub(super) const MOVE_HOME: i32 = 2081;
    pub(super) const MOVE_END: i32 = 2082;
    pub(super) const INSERT: i32 = 2083;
    /// `KEYCODE_F1`; `KEYCODE_F12` follows [`F_KEY_COUNT`](super::F_KEY_COUNT)
    /// codes later.
    pub(super) const F1: i32 = 2090;
}

/// Number of function keys the code range covers, `F1` through `F12`.
const F_KEY_COUNT: u8 = 12;

/// Digit and punctuation keys whose shifted form produces a different key name,
/// as `(unshifted, shifted, key code)`.
///
/// The shifted names must stay identical to the ones
/// [`super::keycodes`] derives for the same keys, because that is where the
/// keystrokes the settings editor records come from.
const SHIFTED_KEYS: [(char, char, i32); 21] = [
    ('0', '!', keycode::KEY_0),
    ('1', '@', keycode::KEY_0 + 1),
    ('2', '#', keycode::KEY_0 + 2),
    ('3', '$', keycode::KEY_0 + 3),
    ('4', '%', keycode::KEY_0 + 4),
    ('5', '^', keycode::KEY_0 + 5),
    ('6', '&', keycode::KEY_0 + 6),
    ('7', '*', keycode::KEY_0 + 7),
    ('8', '(', keycode::KEY_0 + 8),
    ('9', ')', keycode::KEY_0 + 9),
    ('`', '~', keycode::GRAVE),
    ('-', '_', keycode::MINUS),
    ('=', '+', keycode::EQUALS),
    ('[', '{', keycode::LEFT_BRACKET),
    (']', '}', keycode::RIGHT_BRACKET),
    ('\\', '|', keycode::BACKSLASH),
    (';', ':', keycode::SEMICOLON),
    ('\'', '"', keycode::APOSTROPHE),
    ('/', '?', keycode::SLASH),
    (',', '<', keycode::COMMA),
    ('.', '>', keycode::PERIOD),
];

/// Named keys and their codes, as warpui spells them.
const NAMED_KEYS: [(&str, i32); 15] = [
    (" ", keycode::SPACE),
    ("enter", keycode::ENTER),
    ("tab", keycode::TAB),
    ("backspace", keycode::DEL),
    ("delete", keycode::FORWARD_DEL),
    ("escape", keycode::ESCAPE),
    ("home", keycode::MOVE_HOME),
    ("end", keycode::MOVE_END),
    ("insert", keycode::INSERT),
    ("pageup", keycode::PAGE_UP),
    ("pagedown", keycode::PAGE_DOWN),
    ("up", keycode::DPAD_UP),
    ("down", keycode::DPAD_DOWN),
    ("left", keycode::DPAD_LEFT),
    ("right", keycode::DPAD_RIGHT),
];

/// Converts a keystroke into the modifier keys and final key the OHOS input
/// service registers it under.
///
/// Returns `None`, after logging why, when the keystroke cannot be expressed as
/// an OHOS hotkey: no modifier key, more modifier keys than the service
/// accepts, or a key name outside the tables above.
pub(super) fn keystroke_to_hotkey(keystroke: &Keystroke) -> Option<(Vec<i32>, i32)> {
    let mut pre_keys = Vec::new();
    if keystroke.ctrl {
        pre_keys.push(keycode::CTRL_LEFT);
    }
    if keystroke.alt {
        pre_keys.push(keycode::ALT_LEFT);
    }
    if keystroke.shift {
        pre_keys.push(keycode::SHIFT_LEFT);
    }
    // `Keystroke::meta` is folded into `alt` before a shortcut reaches the
    // platform layer, so it never appears here; `cmd` has no OHOS equivalent
    // other than the Meta key.
    if keystroke.cmd {
        pre_keys.push(keycode::META_LEFT);
    }

    if pre_keys.is_empty() {
        log::warn!("ohos::hotkey: '{keystroke:?}' has no modifier key, which OHOS requires");
        return None;
    }
    if pre_keys.len() > MAX_PRE_KEYS {
        log::warn!(
            "ohos::hotkey: '{keystroke:?}' combines {} modifier keys, more than the {MAX_PRE_KEYS} \
             OHOS accepts",
            pre_keys.len()
        );
        return None;
    }

    let Some(final_key) = keycode_from_key_name(&keystroke.key) else {
        log::warn!(
            "ohos::hotkey: '{}' is not a key name the OHOS hotkey service can register",
            keystroke.key
        );
        return None;
    };

    log::info!(
        "ohos::hotkey: mapped '{keystroke:?}' onto pre_keys={pre_keys:?} final_key={final_key}"
    );
    Some((pre_keys, final_key))
}

/// Rebuilds the keystroke a fired hotkey was registered as.
///
/// The modifier list and final key come back from the service as codes, so
/// they are mapped through the same tables [`keystroke_to_hotkey`] used.
/// Returns `None`, after logging why, when a code is not part of a warpui
/// keystroke — the caller then reports the shortcut as unrecognised instead of
/// dispatching a wrong one.
pub(super) fn hotkey_to_keystroke(pre_keys: &[i32], final_key: i32) -> Option<Keystroke> {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut cmd = false;
    for code in pre_keys {
        match *code {
            keycode::CTRL_LEFT | keycode::CTRL_RIGHT => ctrl = true,
            keycode::ALT_LEFT | keycode::ALT_RIGHT => alt = true,
            keycode::SHIFT_LEFT | keycode::SHIFT_RIGHT => shift = true,
            keycode::META_LEFT | keycode::META_RIGHT => cmd = true,
            unknown => {
                log::warn!("ohos::hotkey: {unknown} is not a modifier key the back-end maps");
                return None;
            }
        }
    }

    let Some(key) = key_name_from_keycode(final_key, shift) else {
        log::warn!("ohos::hotkey: {final_key} is not a key code the back-end maps");
        return None;
    };

    let keystroke = Keystroke {
        ctrl,
        alt,
        shift,
        cmd,
        meta: false,
        key,
    };
    log::info!("ohos::hotkey: rebuilt '{keystroke:?}' from the fired hotkey");
    Some(keystroke)
}

/// The key code for a warpui key name, accepting both the unshifted and the
/// shifted spelling.
fn keycode_from_key_name(name: &str) -> Option<i32> {
    if let Some(character) = single_char(name) {
        if let Some(code) = keycode_from_char(character) {
            return Some(code);
        }
    }
    if let Some(code) = named_keycode(name) {
        return Some(code);
    }
    function_key_number(name).map(|number| keycode::F1 + i32::from(number) - 1)
}

/// The key code of a character key, whether or not the character is the shifted
/// form of the physical key.
fn keycode_from_char(character: char) -> Option<i32> {
    let lower = character.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        return Some(keycode::A + (lower as i32 - 'a' as i32));
    }
    SHIFTED_KEYS
        .iter()
        .find(|(plain, shifted, _)| *plain == character || *shifted == character)
        .map(|(_, _, code)| *code)
}

/// The key code of a named key.
fn named_keycode(name: &str) -> Option<i32> {
    NAMED_KEYS
        .iter()
        .find(|(key_name, _)| *key_name == name)
        .map(|(_, code)| *code)
}

/// The number of an `f1`..`f12` key name.
fn function_key_number(name: &str) -> Option<u8> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (1..=F_KEY_COUNT).contains(&number).then_some(number)
}

/// The warpui key name of a key code, in the shifted form when shift is held.
fn key_name_from_keycode(code: i32, shift: bool) -> Option<String> {
    if let Some((plain, shifted, _)) = SHIFTED_KEYS.iter().find(|(_, _, key)| *key == code) {
        return Some(if shift { *shifted } else { *plain }.to_string());
    }
    if let Some((name, _)) = NAMED_KEYS.iter().find(|(_, key)| *key == code) {
        return Some((*name).to_string());
    }
    let last_function_key = keycode::F1 + i32::from(F_KEY_COUNT) - 1;
    if (keycode::F1..=last_function_key).contains(&code) {
        return Some(format!("f{}", code - keycode::F1 + 1));
    }
    if (keycode::A..=keycode::Z).contains(&code) {
        let letter = char::from(b'a' + (code - keycode::A) as u8);
        return Some(if shift {
            letter.to_ascii_uppercase()
        } else {
            letter
        }
        .to_string());
    }
    None
}

/// The character of a one-character name.
fn single_char(name: &str) -> Option<char> {
    let mut characters = name.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Some(character),
        _ => None,
    }
}
