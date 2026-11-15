//! RDP sends PC/AT "set 1" scancodes. Translate them to Linux evdev key
//! codes, which is what the Wayland virtual keyboard expects.

/// Map an RDP scancode (with the E0 "extended" flag) to an evdev key code.
pub fn scancode_to_evdev(code: u8, extended: bool) -> Option<u32> {
    if !extended {
        // For the base set, evdev codes 1..=88 are numerically identical
        // to set-1 scancodes (KEY_ESC = 1 ... KEY_F12 = 88).
        return match code {
            0x01..=0x58 => Some(u32::from(code)),
            0x59 => Some(117), // KP =
            0x64..=0x6e => Some(u32::from(code) - 0x64 + 183), // F13..F23
            0x70 => Some(93),  // Katakana/Hiragana
            0x73 => Some(89),  // RO
            0x79 => Some(92),  // Henkan
            0x7b => Some(94),  // Muhenkan
            0x7d => Some(124), // Yen
            _ => None,
        };
    }
    Some(match code {
        0x1c => 96,  // KP Enter
        0x1d => 97,  // Right Ctrl
        0x35 => 98,  // KP /
        0x37 => 99,  // Print Screen
        0x38 => 100, // Right Alt (AltGr)
        0x46 => 119, // Pause (Ctrl+Break)
        0x47 => 102, // Home
        0x48 => 103, // Up
        0x49 => 104, // Page Up
        0x4b => 105, // Left
        0x4d => 106, // Right
        0x4f => 107, // End
        0x50 => 108, // Down
        0x51 => 109, // Page Down
        0x52 => 110, // Insert
        0x53 => 111, // Delete
        0x5b => 125, // Left Super
        0x5c => 126, // Right Super
        0x5d => 127, // Menu
        0x5e => 116, // Power
        0x5f => 142, // Sleep
        0x20 => 113, // Mute
        0x2e => 114, // Volume down
        0x30 => 115, // Volume up
        0x22 => 164, // Play/Pause
        0x24 => 166, // Stop
        0x10 => 165, // Previous track
        0x19 => 163, // Next track
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(scancode_to_evdev(0x1e, false), Some(30)); // A
        assert_eq!(scancode_to_evdev(0x1c, false), Some(28)); // Enter
        assert_eq!(scancode_to_evdev(0x1c, true), Some(96)); // KP Enter
        assert_eq!(scancode_to_evdev(0x48, true), Some(103)); // Up
        assert_eq!(scancode_to_evdev(0x5b, true), Some(125)); // Super
        assert_eq!(scancode_to_evdev(0xff, false), None);
    }
}
