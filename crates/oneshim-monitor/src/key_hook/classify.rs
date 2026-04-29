//! Pure function to classify platform keycodes into KeyCategory.
//!
//! Each platform calls this with its native keycode. The function maps
//! the code to one of: Enter, Tab, Arrow, Backspace, Special, Regular.

use oneshim_core::models::app_registry::KeyCategory;

// -- macOS CGKeyCode constants --
// Ref: /System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/
//      HIToolbox.framework/Headers/Events.h

/// Classify a macOS CGKeyCode into a KeyCategory.
#[cfg(target_os = "macos")]
pub fn classify_keycode(keycode: u16) -> KeyCategory {
    match keycode {
        // Return / Enter
        36 | 76 => KeyCategory::Enter,
        // Tab
        48 => KeyCategory::Tab,
        // Arrow keys
        123..=126 => KeyCategory::Arrow,
        // Delete (backspace) / Forward Delete
        51 | 117 => KeyCategory::Backspace,
        // Escape
        53 => KeyCategory::Special,
        // Home / End / Page Up / Page Down
        115 | 119 | 116 | 121 => KeyCategory::Special,
        // Function keys F1-F20
        122 | 120 | 99 | 118 | 96 | 97 | 98 | 100 | 101 | 109 | 103 | 111 | 105 | 107 | 113
        | 106 | 64 | 79 | 80 | 90 => KeyCategory::Special,
        // Modifier keys (Shift, Control, Option, Command, Caps Lock, Fn)
        54..=63 => KeyCategory::Special,
        // Everything else is Regular
        _ => KeyCategory::Regular,
    }
}

// -- Windows Virtual Key codes --
// Ref: https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes

/// Classify a Windows virtual key code into a KeyCategory.
#[cfg(target_os = "windows")]
pub fn classify_keycode(vk_code: u32) -> KeyCategory {
    match vk_code {
        // VK_RETURN (0x0D)
        0x0D => KeyCategory::Enter,
        // VK_TAB (0x09)
        0x09 => KeyCategory::Tab,
        // VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN (0x25-0x28)
        0x25..=0x28 => KeyCategory::Arrow,
        // VK_BACK (0x08), VK_DELETE (0x2E)
        0x08 | 0x2E => KeyCategory::Backspace,
        // VK_ESCAPE (0x1B)
        0x1B => KeyCategory::Special,
        // VK_HOME (0x24), VK_END (0x23), VK_PRIOR/PageUp (0x21), VK_NEXT/PageDown (0x22)
        0x21..=0x24 => KeyCategory::Special,
        // VK_F1-VK_F24 (0x70-0x87)
        0x70..=0x87 => KeyCategory::Special,
        // VK_SHIFT, VK_CONTROL, VK_MENU (Alt), VK_LWIN, VK_RWIN
        0x10..=0x12 | 0x5B | 0x5C => KeyCategory::Special,
        // VK_CAPITAL (Caps Lock), VK_NUMLOCK, VK_SCROLL
        0x14 | 0x90 | 0x91 => KeyCategory::Special,
        // Everything else
        _ => KeyCategory::Regular,
    }
}

// -- Linux X11 keysym constants --
// Ref: /usr/include/X11/keysymdef.h

/// Classify a Linux X11 keysym into a KeyCategory.
#[cfg(target_os = "linux")]
pub fn classify_keycode(keysym: u32) -> KeyCategory {
    // XK_Return = 0xFF0D, XK_KP_Enter = 0xFF8D
    const XK_RETURN: u32 = 0xFF0D;
    const XK_KP_ENTER: u32 = 0xFF8D;
    const XK_TAB: u32 = 0xFF09;
    const XK_LEFT: u32 = 0xFF51;
    const XK_UP: u32 = 0xFF52;
    const XK_RIGHT: u32 = 0xFF53;
    const XK_DOWN: u32 = 0xFF54;
    const XK_BACKSPACE: u32 = 0xFF08;
    const XK_DELETE: u32 = 0xFFFF;
    const XK_ESCAPE: u32 = 0xFF1B;
    const XK_HOME: u32 = 0xFF50;
    const XK_END: u32 = 0xFF57;
    const XK_PAGE_UP: u32 = 0xFF55;
    const XK_PAGE_DOWN: u32 = 0xFF56;
    const XK_F1: u32 = 0xFFBE;
    const XK_F24: u32 = 0xFFD5;
    // Modifier range: XK_Shift_L (0xFFE1) through XK_Hyper_R (0xFFEE)
    const XK_SHIFT_L: u32 = 0xFFE1;
    const XK_HYPER_R: u32 = 0xFFEE;
    const XK_CAPS_LOCK: u32 = 0xFFE5;

    match keysym {
        XK_RETURN | XK_KP_ENTER => KeyCategory::Enter,
        XK_TAB => KeyCategory::Tab,
        XK_LEFT | XK_UP | XK_RIGHT | XK_DOWN => KeyCategory::Arrow,
        XK_BACKSPACE | XK_DELETE => KeyCategory::Backspace,
        XK_ESCAPE | XK_HOME | XK_END | XK_PAGE_UP | XK_PAGE_DOWN => KeyCategory::Special,
        k if (XK_F1..=XK_F24).contains(&k) => KeyCategory::Special,
        k if (XK_SHIFT_L..=XK_HYPER_R).contains(&k) => KeyCategory::Special,
        XK_CAPS_LOCK => KeyCategory::Special,
        _ => KeyCategory::Regular,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- macOS tests --

    #[cfg(target_os = "macos")]
    mod macos_tests {
        use super::*;

        #[test]
        fn return_key_is_enter() {
            assert_eq!(classify_keycode(36), KeyCategory::Enter);
        }

        #[test]
        fn numpad_enter_is_enter() {
            assert_eq!(classify_keycode(76), KeyCategory::Enter);
        }

        #[test]
        fn tab_key() {
            assert_eq!(classify_keycode(48), KeyCategory::Tab);
        }

        #[test]
        fn arrow_keys() {
            for code in [123, 124, 125, 126] {
                assert_eq!(classify_keycode(code), KeyCategory::Arrow, "keycode {code}");
            }
        }

        #[test]
        fn delete_is_backspace() {
            assert_eq!(classify_keycode(51), KeyCategory::Backspace);
        }

        #[test]
        fn forward_delete_is_backspace() {
            assert_eq!(classify_keycode(117), KeyCategory::Backspace);
        }

        #[test]
        fn escape_is_special() {
            assert_eq!(classify_keycode(53), KeyCategory::Special);
        }

        #[test]
        fn function_key_f1_is_special() {
            assert_eq!(classify_keycode(122), KeyCategory::Special);
        }

        #[test]
        fn modifier_keys_are_special() {
            // Shift (56), Control (59), Option (58), Command (55)
            for code in [56, 59, 58, 55] {
                assert_eq!(
                    classify_keycode(code),
                    KeyCategory::Special,
                    "keycode {code}"
                );
            }
        }

        #[test]
        fn letter_keys_are_regular() {
            // 'A' = keycode 0, 'S' = 1, ...
            for code in [0, 1, 2, 3, 11, 12, 13, 14] {
                assert_eq!(
                    classify_keycode(code),
                    KeyCategory::Regular,
                    "keycode {code}"
                );
            }
        }

        #[test]
        fn number_keys_are_regular() {
            // 1-0 on main keyboard: 18-29
            for code in 18..=29 {
                assert_eq!(
                    classify_keycode(code),
                    KeyCategory::Regular,
                    "keycode {code}"
                );
            }
        }

        #[test]
        fn home_end_pageup_pagedown_are_special() {
            for code in [115, 119, 116, 121] {
                assert_eq!(
                    classify_keycode(code),
                    KeyCategory::Special,
                    "keycode {code}"
                );
            }
        }
    }

    // -- Windows tests --

    #[cfg(target_os = "windows")]
    mod windows_tests {
        use super::*;

        #[test]
        fn vk_return_is_enter() {
            assert_eq!(classify_keycode(0x0D), KeyCategory::Enter);
        }

        #[test]
        fn vk_tab_is_tab() {
            assert_eq!(classify_keycode(0x09), KeyCategory::Tab);
        }

        #[test]
        fn vk_arrows() {
            for code in [0x25, 0x26, 0x27, 0x28] {
                assert_eq!(classify_keycode(code), KeyCategory::Arrow, "vk {code:#x}");
            }
        }

        #[test]
        fn vk_back_is_backspace() {
            assert_eq!(classify_keycode(0x08), KeyCategory::Backspace);
        }

        #[test]
        fn vk_delete_is_backspace() {
            assert_eq!(classify_keycode(0x2E), KeyCategory::Backspace);
        }

        #[test]
        fn vk_escape_is_special() {
            assert_eq!(classify_keycode(0x1B), KeyCategory::Special);
        }

        #[test]
        fn vk_f1_is_special() {
            assert_eq!(classify_keycode(0x70), KeyCategory::Special);
        }

        #[test]
        fn vk_a_is_regular() {
            assert_eq!(classify_keycode(0x41), KeyCategory::Regular);
        }
    }

    // -- Linux tests --

    #[cfg(target_os = "linux")]
    mod linux_tests {
        use super::*;

        #[test]
        fn xk_return_is_enter() {
            assert_eq!(classify_keycode(0xFF0D), KeyCategory::Enter);
        }

        #[test]
        fn xk_kp_enter_is_enter() {
            assert_eq!(classify_keycode(0xFF8D), KeyCategory::Enter);
        }

        #[test]
        fn xk_tab_is_tab() {
            assert_eq!(classify_keycode(0xFF09), KeyCategory::Tab);
        }

        #[test]
        fn xk_arrows() {
            for code in [0xFF51, 0xFF52, 0xFF53, 0xFF54] {
                assert_eq!(
                    classify_keycode(code),
                    KeyCategory::Arrow,
                    "keysym {code:#x}"
                );
            }
        }

        #[test]
        fn xk_backspace_is_backspace() {
            assert_eq!(classify_keycode(0xFF08), KeyCategory::Backspace);
        }

        #[test]
        fn xk_delete_is_backspace() {
            assert_eq!(classify_keycode(0xFFFF), KeyCategory::Backspace);
        }

        #[test]
        fn xk_escape_is_special() {
            assert_eq!(classify_keycode(0xFF1B), KeyCategory::Special);
        }

        #[test]
        fn xk_f1_is_special() {
            assert_eq!(classify_keycode(0xFFBE), KeyCategory::Special);
        }

        #[test]
        fn xk_a_is_regular() {
            // XK_a = 0x0061
            assert_eq!(classify_keycode(0x0061), KeyCategory::Regular);
        }
    }
}
