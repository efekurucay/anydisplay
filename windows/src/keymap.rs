//! USB HID keyboard page (0x07) usage → Windows key, for `SendInput`.

/// How to inject a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinKey {
    /// PS/2 set-1 scan code; `extended` = `E0` prefix (`KEYEVENTF_EXTENDEDKEY`).
    Scan { code: u16, extended: bool },
    /// Keys without a plain scan code (Pause, Num Lock, media keys) go by virtual-key code.
    Vk(u16),
}

const fn s(code: u16) -> Option<WinKey> {
    Some(WinKey::Scan {
        code,
        extended: false,
    })
}

const fn e(code: u16) -> Option<WinKey> {
    Some(WinKey::Scan {
        code,
        extended: true,
    })
}

const fn vk(code: u16) -> Option<WinKey> {
    Some(WinKey::Vk(code))
}

pub fn win_key(hid_usage: u16) -> Option<WinKey> {
    const LETTERS: [u16; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, // a-m
        0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C, // n-z
    ];
    match hid_usage {
        0x04..=0x1D => s(LETTERS[(hid_usage - 0x04) as usize]),
        0x1E..=0x27 => s(hid_usage - 0x1E + 0x02), // 1-9, 0
        0x28 => s(0x1C),                           // Enter
        0x29 => s(0x01),                           // Escape
        0x2A => s(0x0E),                           // Backspace
        0x2B => s(0x0F),                           // Tab
        0x2C => s(0x39),                           // Space
        0x2D => s(0x0C),                           // -
        0x2E => s(0x0D),                           // =
        0x2F => s(0x1A),                           // [
        0x30 => s(0x1B),                           // ]
        0x31 | 0x32 => s(0x2B),                    // \ and non-US #
        0x33 => s(0x27),                           // ;
        0x34 => s(0x28),                           // '
        0x35 => s(0x29),                           // `
        0x36 => s(0x33),                           // ,
        0x37 => s(0x34),                           // .
        0x38 => s(0x35),                           // /
        0x39 => s(0x3A),                           // Caps Lock
        0x3A..=0x43 => s(hid_usage - 0x3A + 0x3B), // F1-F10
        0x44 => s(0x57),                           // F11
        0x45 => s(0x58),                           // F12
        0x46 => e(0x37),                           // Print Screen
        0x47 => s(0x46),                           // Scroll Lock
        0x48 => vk(0x13),                          // Pause (VK_PAUSE)
        0x49 => e(0x52),                           // Insert
        0x4A => e(0x47),                           // Home
        0x4B => e(0x49),                           // Page Up
        0x4C => e(0x53),                           // Delete
        0x4D => e(0x4F),                           // End
        0x4E => e(0x51),                           // Page Down
        0x4F => e(0x4D),                           // Right
        0x50 => e(0x4B),                           // Left
        0x51 => e(0x50),                           // Down
        0x52 => e(0x48),                           // Up
        0x53 => vk(0x90),                          // Num Lock (VK_NUMLOCK)
        0x54 => e(0x35),                           // KP /
        0x55 => s(0x37),                           // KP *
        0x56 => s(0x4A),                           // KP -
        0x57 => s(0x4E),                           // KP +
        0x58 => e(0x1C),                           // KP Enter
        0x59 => s(0x4F),                           // KP 1
        0x5A => s(0x50),                           // KP 2
        0x5B => s(0x51),                           // KP 3
        0x5C => s(0x4B),                           // KP 4
        0x5D => s(0x4C),                           // KP 5
        0x5E => s(0x4D),                           // KP 6
        0x5F => s(0x47),                           // KP 7
        0x60 => s(0x48),                           // KP 8
        0x61 => s(0x49),                           // KP 9
        0x62 => s(0x52),                           // KP 0
        0x63 => s(0x53),                           // KP .
        0x64 => s(0x56),                           // Non-US backslash
        0x65 => e(0x5D),                           // Application (menu)
        0x67 => s(0x59),                           // KP =
        0x68..=0x72 => s(hid_usage - 0x68 + 0x64), // F13-F23
        0x73 => s(0x76),                           // F24
        0x7F => vk(0xAD),                          // Mute
        0x80 => vk(0xAF),                          // Volume Up
        0x81 => vk(0xAE),                          // Volume Down
        0x87 => s(0x73),                           // International1 (Ro)
        0x88 => s(0x70),                           // International2 (Katakana/Hiragana)
        0x89 => s(0x7D),                           // International3 (Yen)
        0x8A => s(0x79),                           // International4 (Henkan)
        0x8B => s(0x7B),                           // International5 (Muhenkan)
        0x90 => s(0xF2),                           // LANG1 (Hangul / Kana on)
        0x91 => s(0xF1),                           // LANG2 (Hanja / Eisu)
        0xE0 => s(0x1D),                           // Left Ctrl
        0xE1 => s(0x2A),                           // Left Shift
        0xE2 => s(0x38),                           // Left Alt
        0xE3 => e(0x5B),                           // Left Windows
        0xE4 => e(0x1D),                           // Right Ctrl
        0xE5 => s(0x36),                           // Right Shift
        0xE6 => e(0x38),                           // Right Alt
        0xE7 => e(0x5C),                           // Right Windows
        _ => None,
    }
}

pub fn is_modifier(hid_usage: u16) -> bool {
    (0xE0..=0xE7).contains(&hid_usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(
            win_key(0x04),
            Some(WinKey::Scan {
                code: 0x1E,
                extended: false
            })
        ); // a
        assert_eq!(
            win_key(0x1D),
            Some(WinKey::Scan {
                code: 0x2C,
                extended: false
            })
        ); // z
        assert_eq!(
            win_key(0x27),
            Some(WinKey::Scan {
                code: 0x0B,
                extended: false
            })
        ); // 0
        assert_eq!(
            win_key(0x43),
            Some(WinKey::Scan {
                code: 0x44,
                extended: false
            })
        ); // F10
        assert_eq!(
            win_key(0x52),
            Some(WinKey::Scan {
                code: 0x48,
                extended: true
            })
        ); // Up
        assert_eq!(
            win_key(0x6F),
            Some(WinKey::Scan {
                code: 0x6B,
                extended: false
            })
        ); // F20
        assert_eq!(
            win_key(0xE7),
            Some(WinKey::Scan {
                code: 0x5C,
                extended: true
            })
        );
        assert_eq!(win_key(0x48), Some(WinKey::Vk(0x13)));
        assert_eq!(win_key(0x00), None);
        // Every letter maps to a distinct scan code.
        let mut codes: Vec<_> = (0x04..=0x1D).map(|u| win_key(u).unwrap()).collect();
        codes.dedup();
        assert_eq!(codes.len(), 26);
    }
}
