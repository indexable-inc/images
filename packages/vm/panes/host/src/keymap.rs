//! macOS virtual keycode (kVK, `NSEvent.keyCode`) -> evdev keycode.
//!
//! GENERATED FILE, do not edit by hand. Produced by `tools/gen_keymap.py`
//! from the keycodemapdb project's `data/keymaps.csv` ("OS-X Keycode"
//! column -> "Linux Keycode" column), the dataset QEMU and libvirt derive
//! their keymaps from:
//!
//!     https://gitlab.com/keycodemap/keycodemapdb/-/raw/master/data/keymaps.csv
//!
//! Regenerate with: `python3 tools/gen_keymap.py > src/keymap.rs`

/// Dense kVK -> evdev table. kVK codes are 7-bit; 0 marks "no mapping"
/// (`KEY_RESERVED`, which never travels the wire).
const KVK_TO_EVDEV: [u16; 128] = [
    30, // 0x00 ANSI_A -> KEY_A
    31, // 0x01 ANSI_S -> KEY_S
    32, // 0x02 ANSI_D -> KEY_D
    33, // 0x03 ANSI_F -> KEY_F
    35, // 0x04 ANSI_H -> KEY_H
    34, // 0x05 ANSI_G -> KEY_G
    44, // 0x06 ANSI_Z -> KEY_Z
    45, // 0x07 ANSI_X -> KEY_X
    46, // 0x08 ANSI_C -> KEY_C
    47, // 0x09 ANSI_V -> KEY_V
    86, // 0x0a ISO_Section -> KEY_102ND
    48, // 0x0b ANSI_B -> KEY_B
    16, // 0x0c ANSI_Q -> KEY_Q
    17, // 0x0d ANSI_W -> KEY_W
    18, // 0x0e ANSI_E -> KEY_E
    19, // 0x0f ANSI_R -> KEY_R
    21, // 0x10 ANSI_Y -> KEY_Y
    20, // 0x11 ANSI_T -> KEY_T
    2, // 0x12 ANSI_1 -> KEY_1
    3, // 0x13 ANSI_2 -> KEY_2
    4, // 0x14 ANSI_3 -> KEY_3
    5, // 0x15 ANSI_4 -> KEY_4
    7, // 0x16 ANSI_6 -> KEY_6
    6, // 0x17 ANSI_5 -> KEY_5
    13, // 0x18 ANSI_Equal -> KEY_EQUAL
    10, // 0x19 ANSI_9 -> KEY_9
    8, // 0x1a ANSI_7 -> KEY_7
    12, // 0x1b ANSI_Minus -> KEY_MINUS
    9, // 0x1c ANSI_8 -> KEY_8
    11, // 0x1d ANSI_0 -> KEY_0
    27, // 0x1e ANSI_RightBracket -> KEY_RIGHTBRACE
    24, // 0x1f ANSI_O -> KEY_O
    22, // 0x20 ANSI_U -> KEY_U
    26, // 0x21 ANSI_LeftBracket -> KEY_LEFTBRACE
    23, // 0x22 ANSI_I -> KEY_I
    25, // 0x23 ANSI_P -> KEY_P
    28, // 0x24 Return -> KEY_ENTER
    38, // 0x25 ANSI_L -> KEY_L
    36, // 0x26 ANSI_J -> KEY_J
    40, // 0x27 ANSI_Quote -> KEY_APOSTROPHE
    37, // 0x28 ANSI_K -> KEY_K
    39, // 0x29 ANSI_Semicolon -> KEY_SEMICOLON
    43, // 0x2a ANSI_Backslash -> KEY_BACKSLASH
    51, // 0x2b ANSI_Comma -> KEY_COMMA
    53, // 0x2c ANSI_Slash -> KEY_SLASH
    49, // 0x2d ANSI_N -> KEY_N
    50, // 0x2e ANSI_M -> KEY_M
    52, // 0x2f ANSI_Period -> KEY_DOT
    15, // 0x30 Tab -> KEY_TAB
    57, // 0x31 Space -> KEY_SPACE
    41, // 0x32 ANSI_Grave -> KEY_GRAVE
    14, // 0x33 Delete -> KEY_BACKSPACE
    0, // 0x34 (unmapped)
    1, // 0x35 Escape -> KEY_ESC
    126, // 0x36 RightCommand -> KEY_RIGHTMETA
    125, // 0x37 Command -> KEY_LEFTMETA
    42, // 0x38 Shift -> KEY_SHIFT
    58, // 0x39 CapsLock -> KEY_CAPSLOCK
    56, // 0x3a Option -> KEY_LEFTALT
    29, // 0x3b Control -> KEY_LEFTCTRL
    54, // 0x3c RightShift -> KEY_RIGHTSHIFT
    100, // 0x3d RightOption -> KEY_RIGHTALT
    97, // 0x3e RightControl -> KEY_RIGHTCTRL
    464, // 0x3f Function -> KEY_FN
    187, // 0x40 F17 -> KEY_F17
    83, // 0x41 ANSI_KeypadDecimal -> KEY_KPDOT
    0, // 0x42 (unmapped)
    55, // 0x43 ANSI_KeypadMultiply -> KEY_KPASTERISK
    0, // 0x44 (unmapped)
    78, // 0x45 ANSI_KeypadPlus -> KEY_KPPLUS
    0, // 0x46 (unmapped)
    69, // 0x47 ANSI_KeypadClear -> KEY_NUMLOCK
    115, // 0x48 VolumeUp -> KEY_VOLUMEUP
    114, // 0x49 VolumeDown -> KEY_VOLUMEDOWN
    113, // 0x4a Mute -> KEY_MUTE
    98, // 0x4b ANSI_KeypadDivide -> KEY_KPSLASH
    96, // 0x4c ANSI_KeypadEnter -> KEY_KPENTER
    0, // 0x4d (unmapped)
    74, // 0x4e ANSI_KeypadMinus -> KEY_KPMINUS
    188, // 0x4f F18 -> KEY_F18
    189, // 0x50 F19 -> KEY_F19
    117, // 0x51 ANSI_KeypadEquals -> KEY_KPEQUAL
    82, // 0x52 ANSI_Keypad0 -> KEY_KP0
    79, // 0x53 ANSI_Keypad1 -> KEY_KP1
    80, // 0x54 ANSI_Keypad2 -> KEY_KP2
    81, // 0x55 ANSI_Keypad3 -> KEY_KP3
    75, // 0x56 ANSI_Keypad4 -> KEY_KP4
    76, // 0x57 ANSI_Keypad5 -> KEY_KP5
    77, // 0x58 ANSI_Keypad6 -> KEY_KP6
    71, // 0x59 ANSI_Keypad7 -> KEY_KP7
    190, // 0x5a F20 -> KEY_F20
    72, // 0x5b ANSI_Keypad8 -> KEY_KP8
    73, // 0x5c ANSI_Keypad9 -> KEY_KP9
    124, // 0x5d JIS_Yen -> KEY_YEN
    89, // 0x5e JIS_Underscore -> KEY_RO
    95, // 0x5f JIS_KeypadComma -> KEY_KPJPCOMMA
    63, // 0x60 F5 -> KEY_F5
    64, // 0x61 F6 -> KEY_F6
    65, // 0x62 F7 -> KEY_F7
    61, // 0x63 F3 -> KEY_F3
    66, // 0x64 F8 -> KEY_F8
    67, // 0x65 F9 -> KEY_F9
    123, // 0x66 JIS_Eisu -> KEY_HANJA
    87, // 0x67 F11 -> KEY_F11
    122, // 0x68 JIS_Kana -> KEY_HANGEUL
    183, // 0x69 F13 -> KEY_F13
    186, // 0x6a F16 -> KEY_F16
    184, // 0x6b F14 -> KEY_F14
    0, // 0x6c (unmapped)
    68, // 0x6d F10 -> KEY_F10
    127, // 0x6e  -> KEY_COMPOSE
    88, // 0x6f F12 -> KEY_F12
    0, // 0x70 (unmapped)
    185, // 0x71 F15 -> KEY_F15
    138, // 0x72 Help -> KEY_HELP
    102, // 0x73 Home -> KEY_HOME
    104, // 0x74 PageUp -> KEY_PAGEUP
    111, // 0x75 ForwardDelete -> KEY_DELETE
    62, // 0x76 F4 -> KEY_F4
    107, // 0x77 End -> KEY_END
    60, // 0x78 F2 -> KEY_F2
    109, // 0x79 PageDown -> KEY_PAGEDOWN
    59, // 0x7a F1 -> KEY_F1
    105, // 0x7b LeftArrow -> KEY_LEFT
    106, // 0x7c RightArrow -> KEY_RIGHT
    108, // 0x7d DownArrow -> KEY_DOWN
    103, // 0x7e UpArrow -> KEY_UP
    0, // 0x7f (unmapped)
];

/// The evdev keycode for a macOS virtual keycode, `None` when unmapped.
pub fn evdev_from_kvk(kvk: u16) -> Option<u32> {
    let code = *KVK_TO_EVDEV.get(usize::from(kvk))?;
    (code != 0).then_some(u32::from(code))
}

#[cfg(test)]
mod tests {
    use super::evdev_from_kvk;

    #[test]
    fn spot_checks_against_keycodemapdb() {
        assert_eq!(evdev_from_kvk(0x00), Some(30)); // ANSI_A -> KEY_A
        assert_eq!(evdev_from_kvk(0x24), Some(28)); // Return -> KEY_ENTER
        assert_eq!(evdev_from_kvk(0x35), Some(1)); // Escape -> KEY_ESC
        assert_eq!(evdev_from_kvk(0x7E), Some(103)); // UpArrow -> KEY_UP
        assert_eq!(evdev_from_kvk(0x39), Some(58)); // CapsLock -> KEY_CAPSLOCK
    }

    #[test]
    fn out_of_range_and_unmapped_are_none() {
        assert_eq!(evdev_from_kvk(0x80), None);
        assert_eq!(evdev_from_kvk(u16::MAX), None);
    }
}
