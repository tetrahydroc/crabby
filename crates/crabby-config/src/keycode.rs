//! Godot 4 keycode -> human label mapping.
//!
//! Ported from `core/os/keyboard.h` in Godot 4.6.2. The enum is stable
//! across the 4.x line; if a future Godot adds a new key, only the
//! label is missed, never the round-trip (the raw integer is always
//! echoed back to MCM's config.ini).
//!
//! # Mouse buttons
//!
//! MCM's keycode capture accepts both `InputEventKey` and
//! `InputEventMouseButton`, but stores both via the
//! `physical_keycode` field. Godot's `InputEventMouseButton` doesn't
//! have a `physical_keycode` (assigning to it is a no-op), so the
//! current MCM build silently drops mouse-button binds (writes 0).
//!
//! Crabby treats that as an MCM bug to be fixed and pre-emptively
//! claims the unused `1<<23` bit in Godot's keycode space as a "this
//! is a mouse button" flag. Layout:
//!
//! - bit 22 (`SPECIAL`), Godot's special-key flag
//! - bit 23 (`MOUSE_FLAG`), crabby extension; low bits = MouseButton index
//! - bits 24+, Godot's modifier mask, untouched
//!
//! Real Godot keycodes never set bit 23, so a future MCM that wants to
//! adopt this scheme can detect mouse binds with `code & 0x800000`.

#![allow(missing_docs)]

/// Godot's `Key::SPECIAL` bit.
pub const KEY_SPECIAL: i64 = 1 << 22;

/// Crabby extension: bit 23 marks a mouse-button bind. Low bits hold
/// the Godot `MouseButton` enum value (1=Left, 2=Right, 3=Middle, etc).
pub const MOUSE_FLAG: i64 = 1 << 23;

/// `MouseButton` enum values from Godot's `core/os/keyboard.h` /
/// `core/input/input_event.h`. Only the values that have a stable
/// label across platforms; extras (X1/X2 buttons) are still
/// representable but render as "Mouse N".
pub const MB_LEFT: i64 = 1;
pub const MB_RIGHT: i64 = 2;
pub const MB_MIDDLE: i64 = 3;
pub const MB_WHEEL_UP: i64 = 4;
pub const MB_WHEEL_DOWN: i64 = 5;
pub const MB_WHEEL_LEFT: i64 = 6;
pub const MB_WHEEL_RIGHT: i64 = 7;
pub const MB_XBUTTON1: i64 = 8;
pub const MB_XBUTTON2: i64 = 9;

/// Convert a stored MCM keycode integer into a human label.
///
/// Returns `"None"` for `0`, the mouse label for any value with bit 23
/// set, the named special-key label for known Godot specials, the
/// printable character for ASCII-range keycodes, and a `"#0xN"` fallback
/// for anything not recognised.
#[must_use]
pub fn keycode_label(code: i64) -> String {
    if code == 0 {
        return "None".into();
    }
    if code & MOUSE_FLAG != 0 {
        return mouse_label(code & 0xFFFF);
    }
    // Strip modifier bits (callers may pass a raw event code).
    let bare = code & ((1 << 23) - 1);
    if let Some(name) = SPECIAL_KEYS.iter().find_map(|(k, n)| (*k == bare).then_some(*n)) {
        return name.into();
    }
    // Printable ASCII range: 0x20..=0x7E. Godot uses uppercase letters
    // for A-Z (0x41..0x5A); render those as the corresponding char.
    if (0x20..=0x7E).contains(&bare) {
        if let Some(c) = char::from_u32(bare as u32) {
            return c.to_string();
        }
    }
    // Latin-1 extras (Yen, Section).
    if bare == 0x00A5 {
        return "¥".into();
    }
    if bare == 0x00A7 {
        return "§".into();
    }
    format!("#{bare:#x}")
}

/// Encode a mouse button index as a stored MCM keycode.
#[must_use]
pub fn encode_mouse(button_index: i64) -> i64 {
    MOUSE_FLAG | (button_index & 0xFFFF)
}

/// True if `code` is a mouse-button encoding (per [`MOUSE_FLAG`]).
#[must_use]
pub fn is_mouse(code: i64) -> bool {
    code & MOUSE_FLAG != 0
}

fn mouse_label(button: i64) -> String {
    match button {
        MB_LEFT => "Mouse Left".into(),
        MB_RIGHT => "Mouse Right".into(),
        MB_MIDDLE => "Mouse Middle".into(),
        MB_WHEEL_UP => "Mouse Wheel Up".into(),
        MB_WHEEL_DOWN => "Mouse Wheel Down".into(),
        MB_WHEEL_LEFT => "Mouse Wheel Left".into(),
        MB_WHEEL_RIGHT => "Mouse Wheel Right".into(),
        MB_XBUTTON1 => "Mouse X1".into(),
        MB_XBUTTON2 => "Mouse X2".into(),
        n => format!("Mouse {n}"),
    }
}

/// Special-key table. Pairs of (raw value, label). The raw value is
/// `KEY_SPECIAL | offset` per Godot's enum layout. Order doesn't
/// matter; lookup by linear scan (table is small).
const SPECIAL_KEYS: &[(i64, &str)] = &[
    (KEY_SPECIAL | 0x01, "Escape"),
    (KEY_SPECIAL | 0x02, "Tab"),
    (KEY_SPECIAL | 0x03, "Backtab"),
    (KEY_SPECIAL | 0x04, "Backspace"),
    (KEY_SPECIAL | 0x05, "Enter"),
    (KEY_SPECIAL | 0x06, "Numpad Enter"),
    (KEY_SPECIAL | 0x07, "Insert"),
    (KEY_SPECIAL | 0x08, "Delete"),
    (KEY_SPECIAL | 0x09, "Pause"),
    (KEY_SPECIAL | 0x0A, "Print"),
    (KEY_SPECIAL | 0x0B, "SysReq"),
    (KEY_SPECIAL | 0x0C, "Clear"),
    (KEY_SPECIAL | 0x0D, "Home"),
    (KEY_SPECIAL | 0x0E, "End"),
    (KEY_SPECIAL | 0x0F, "Left"),
    (KEY_SPECIAL | 0x10, "Up"),
    (KEY_SPECIAL | 0x11, "Right"),
    (KEY_SPECIAL | 0x12, "Down"),
    (KEY_SPECIAL | 0x13, "Page Up"),
    (KEY_SPECIAL | 0x14, "Page Down"),
    (KEY_SPECIAL | 0x15, "Shift"),
    (KEY_SPECIAL | 0x16, "Ctrl"),
    (KEY_SPECIAL | 0x17, "Meta"),
    (KEY_SPECIAL | 0x18, "Alt"),
    (KEY_SPECIAL | 0x19, "Caps Lock"),
    (KEY_SPECIAL | 0x1A, "Num Lock"),
    (KEY_SPECIAL | 0x1B, "Scroll Lock"),
    (KEY_SPECIAL | 0x1C, "F1"),
    (KEY_SPECIAL | 0x1D, "F2"),
    (KEY_SPECIAL | 0x1E, "F3"),
    (KEY_SPECIAL | 0x1F, "F4"),
    (KEY_SPECIAL | 0x20, "F5"),
    (KEY_SPECIAL | 0x21, "F6"),
    (KEY_SPECIAL | 0x22, "F7"),
    (KEY_SPECIAL | 0x23, "F8"),
    (KEY_SPECIAL | 0x24, "F9"),
    (KEY_SPECIAL | 0x25, "F10"),
    (KEY_SPECIAL | 0x26, "F11"),
    (KEY_SPECIAL | 0x27, "F12"),
    (KEY_SPECIAL | 0x28, "F13"),
    (KEY_SPECIAL | 0x29, "F14"),
    (KEY_SPECIAL | 0x2A, "F15"),
    (KEY_SPECIAL | 0x2B, "F16"),
    (KEY_SPECIAL | 0x2C, "F17"),
    (KEY_SPECIAL | 0x2D, "F18"),
    (KEY_SPECIAL | 0x2E, "F19"),
    (KEY_SPECIAL | 0x2F, "F20"),
    (KEY_SPECIAL | 0x30, "F21"),
    (KEY_SPECIAL | 0x31, "F22"),
    (KEY_SPECIAL | 0x32, "F23"),
    (KEY_SPECIAL | 0x33, "F24"),
    (KEY_SPECIAL | 0x42, "Menu"),
    (KEY_SPECIAL | 0x43, "Hyper"),
    (KEY_SPECIAL | 0x45, "Help"),
    (KEY_SPECIAL | 0x48, "Back"),
    (KEY_SPECIAL | 0x49, "Forward"),
    (KEY_SPECIAL | 0x81, "Numpad *"),
    (KEY_SPECIAL | 0x82, "Numpad /"),
    (KEY_SPECIAL | 0x83, "Numpad -"),
    (KEY_SPECIAL | 0x84, "Numpad ."),
    (KEY_SPECIAL | 0x85, "Numpad +"),
    (KEY_SPECIAL | 0x86, "Numpad 0"),
    (KEY_SPECIAL | 0x87, "Numpad 1"),
    (KEY_SPECIAL | 0x88, "Numpad 2"),
    (KEY_SPECIAL | 0x89, "Numpad 3"),
    (KEY_SPECIAL | 0x8A, "Numpad 4"),
    (KEY_SPECIAL | 0x8B, "Numpad 5"),
    (KEY_SPECIAL | 0x8C, "Numpad 6"),
    (KEY_SPECIAL | 0x8D, "Numpad 7"),
    (KEY_SPECIAL | 0x8E, "Numpad 8"),
    (KEY_SPECIAL | 0x8F, "Numpad 9"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_letters_render_as_chars() {
        assert_eq!(keycode_label(0x41), "A");
        assert_eq!(keycode_label(0x50), "P");
        assert_eq!(keycode_label(0x5A), "Z");
        assert_eq!(keycode_label(0x20), " ");
    }

    #[test]
    fn digit_keys_render_as_chars() {
        assert_eq!(keycode_label(0x30), "0");
        assert_eq!(keycode_label(0x39), "9");
    }

    #[test]
    fn specials_render_with_named_labels() {
        assert_eq!(keycode_label(KEY_SPECIAL | 0x01), "Escape");
        assert_eq!(keycode_label(KEY_SPECIAL | 0x20), "F5");
        assert_eq!(keycode_label(KEY_SPECIAL | 0x21), "F6");
        assert_eq!(keycode_label(KEY_SPECIAL | 0x04), "Backspace");
    }

    #[test]
    fn zero_is_none() {
        assert_eq!(keycode_label(0), "None");
    }

    #[test]
    fn mouse_buttons_render_via_flag() {
        assert_eq!(keycode_label(encode_mouse(MB_LEFT)), "Mouse Left");
        assert_eq!(keycode_label(encode_mouse(MB_RIGHT)), "Mouse Right");
        assert_eq!(keycode_label(encode_mouse(MB_WHEEL_UP)), "Mouse Wheel Up");
        assert!(is_mouse(encode_mouse(MB_LEFT)));
        assert!(!is_mouse(0x50));
    }

    #[test]
    fn unknown_keycode_falls_back_to_hex() {
        // Pick something inside the special range but unmapped.
        let synthetic = KEY_SPECIAL | 0x70; // GLOBE, not in the table for now.
        let label = keycode_label(synthetic);
        assert!(label.starts_with('#'), "got {label}");
    }
}
