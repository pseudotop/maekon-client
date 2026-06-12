// Copyright 2014-2021 The winit contributors
// Copyright 2021-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0

use super::KeyEventExtra;
use crate::{
    event::{ElementState, KeyEvent},
    keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NativeKeyCode},
};
use gtk::gdk::{self, prelude::*};
use once_cell::sync::Lazy;
use std::{collections::HashSet, sync::Mutex};

pub type RawKey = gdk::Key;

static KEY_STRINGS: Lazy<Mutex<HashSet<&'static str>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn insert_or_get_key_str(string: String) -> &'static str {
    let mut string_set = KEY_STRINGS.lock().unwrap();
    if let Some(contained) = string_set.get(string.as_str()) {
        return contained;
    }
    let static_str = Box::leak(string.into_boxed_str());
    string_set.insert(static_str);
    static_str
}

#[allow(clippy::just_underscores_and_digits, non_upper_case_globals)]
pub(crate) fn raw_key_to_key(gdk_key: RawKey) -> Option<Key<'static>> {
    match gdk_key {
        gdk::Key::Escape => Some(Key::Escape),
        gdk::Key::BackSpace => Some(Key::Backspace),
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Some(Key::Tab),
        gdk::Key::Return => Some(Key::Enter),
        gdk::Key::Control_L | gdk::Key::Control_R => Some(Key::Control),
        gdk::Key::Alt_L | gdk::Key::Alt_R => Some(Key::Alt),
        gdk::Key::Shift_L | gdk::Key::Shift_R => Some(Key::Shift),
        // TODO: investigate mapping. Map Meta_[LR]?
        gdk::Key::Super_L | gdk::Key::Super_R => Some(Key::Super),
        gdk::Key::Caps_Lock => Some(Key::CapsLock),
        gdk::Key::F1 => Some(Key::F1),
        gdk::Key::F2 => Some(Key::F2),
        gdk::Key::F3 => Some(Key::F3),
        gdk::Key::F4 => Some(Key::F4),
        gdk::Key::F5 => Some(Key::F5),
        gdk::Key::F6 => Some(Key::F6),
        gdk::Key::F7 => Some(Key::F7),
        gdk::Key::F8 => Some(Key::F8),
        gdk::Key::F9 => Some(Key::F9),
        gdk::Key::F10 => Some(Key::F10),
        gdk::Key::F11 => Some(Key::F11),
        gdk::Key::F12 => Some(Key::F12),
        gdk::Key::F13 => Some(Key::F13),
        gdk::Key::F14 => Some(Key::F14),
        gdk::Key::F15 => Some(Key::F15),
        gdk::Key::F16 => Some(Key::F16),
        gdk::Key::F17 => Some(Key::F17),
        gdk::Key::F18 => Some(Key::F18),
        gdk::Key::F19 => Some(Key::F19),
        gdk::Key::F20 => Some(Key::F20),
        gdk::Key::F21 => Some(Key::F21),
        gdk::Key::F22 => Some(Key::F22),
        gdk::Key::F23 => Some(Key::F23),
        gdk::Key::F24 => Some(Key::F24),

        gdk::Key::Print => Some(Key::PrintScreen),
        gdk::Key::Scroll_Lock => Some(Key::ScrollLock),
        // Pause/Break not audio.
        gdk::Key::Pause => Some(Key::Pause),

        gdk::Key::Insert => Some(Key::Insert),
        gdk::Key::Delete => Some(Key::Delete),
        gdk::Key::Home => Some(Key::Home),
        gdk::Key::End => Some(Key::End),
        gdk::Key::Page_Up => Some(Key::PageUp),
        gdk::Key::Page_Down => Some(Key::PageDown),
        gdk::Key::Num_Lock => Some(Key::NumLock),

        gdk::Key::Up => Some(Key::ArrowUp),
        gdk::Key::Down => Some(Key::ArrowDown),
        gdk::Key::Left => Some(Key::ArrowLeft),
        gdk::Key::Right => Some(Key::ArrowRight),
        gdk::Key::Clear => Some(Key::Clear),

        gdk::Key::Menu => Some(Key::ContextMenu),
        gdk::Key::WakeUp => Some(Key::WakeUp),
        gdk::Key::Launch0 => Some(Key::LaunchApplication1),
        gdk::Key::Launch1 => Some(Key::LaunchApplication2),
        gdk::Key::ISO_Level3_Shift => Some(Key::AltGraph),

        gdk::Key::KP_Begin => Some(Key::Clear),
        gdk::Key::KP_Delete => Some(Key::Delete),
        gdk::Key::KP_Down => Some(Key::ArrowDown),
        gdk::Key::KP_End => Some(Key::End),
        gdk::Key::KP_Enter => Some(Key::Enter),
        gdk::Key::KP_F1 => Some(Key::F1),
        gdk::Key::KP_F2 => Some(Key::F2),
        gdk::Key::KP_F3 => Some(Key::F3),
        gdk::Key::KP_F4 => Some(Key::F4),
        gdk::Key::KP_Home => Some(Key::Home),
        gdk::Key::KP_Insert => Some(Key::Insert),
        gdk::Key::KP_Left => Some(Key::ArrowLeft),
        gdk::Key::KP_Page_Down => Some(Key::PageDown),
        gdk::Key::KP_Page_Up => Some(Key::PageUp),
        gdk::Key::KP_Right => Some(Key::ArrowRight),
        // KP_Separator? What does it map to?
        gdk::Key::KP_Tab => Some(Key::Tab),
        gdk::Key::KP_Up => Some(Key::ArrowUp),
        // TODO: more mappings (media etc)
        _ => None,
    }
}

#[allow(clippy::just_underscores_and_digits, non_upper_case_globals)]
pub(crate) fn raw_key_to_location(raw: RawKey) -> KeyLocation {
    match raw {
        gdk::Key::Control_L
        | gdk::Key::Shift_L
        | gdk::Key::Alt_L
        | gdk::Key::Super_L
        | gdk::Key::Meta_L => KeyLocation::Left,
        gdk::Key::Control_R
        | gdk::Key::Shift_R
        | gdk::Key::Alt_R
        | gdk::Key::Super_R
        | gdk::Key::Meta_R => KeyLocation::Right,
        gdk::Key::KP_0
        | gdk::Key::KP_1
        | gdk::Key::KP_2
        | gdk::Key::KP_3
        | gdk::Key::KP_4
        | gdk::Key::KP_5
        | gdk::Key::KP_6
        | gdk::Key::KP_7
        | gdk::Key::KP_8
        | gdk::Key::KP_9
        | gdk::Key::KP_Add
        | gdk::Key::KP_Begin
        | gdk::Key::KP_Decimal
        | gdk::Key::KP_Delete
        | gdk::Key::KP_Divide
        | gdk::Key::KP_Down
        | gdk::Key::KP_End
        | gdk::Key::KP_Enter
        | gdk::Key::KP_Equal
        | gdk::Key::KP_F1
        | gdk::Key::KP_F2
        | gdk::Key::KP_F3
        | gdk::Key::KP_F4
        | gdk::Key::KP_Home
        | gdk::Key::KP_Insert
        | gdk::Key::KP_Left
        | gdk::Key::KP_Multiply
        | gdk::Key::KP_Page_Down
        | gdk::Key::KP_Page_Up
        | gdk::Key::KP_Right
        | gdk::Key::KP_Separator
        | gdk::Key::KP_Space
        | gdk::Key::KP_Subtract
        | gdk::Key::KP_Tab
        | gdk::Key::KP_Up => KeyLocation::Numpad,
        _ => KeyLocation::Standard,
    }
}

const MODIFIER_MAP: &[(Key<'static>, ModifiersState)] = &[
    (Key::Shift, ModifiersState::SHIFT),
    (Key::Alt, ModifiersState::ALT),
    (Key::Control, ModifiersState::CONTROL),
    (Key::Super, ModifiersState::SUPER),
];

// We use keyval/keycode from `EventControllerKey` so modifier changes can be
// emitted before the next key event, matching the other platform backends.
pub(crate) fn get_modifiers(keyval: RawKey, scancode: u16) -> ModifiersState {
    // unicode value
    let unicode = keyval.to_unicode();
    // translate to tao::keyboard::Key
    let key_from_code = raw_key_to_key(keyval).unwrap_or_else(|| {
        if let Some(key) = unicode {
            if key >= ' ' && key != '\x7f' {
                Key::Character(insert_or_get_key_str(key.to_string()))
            } else {
                Key::Unidentified(NativeKeyCode::Gtk(scancode))
            }
        } else {
            Key::Unidentified(NativeKeyCode::Gtk(scancode))
        }
    });
    // start with empty state
    let mut result = ModifiersState::empty();
    // loop trough our modifier map
    for (gdk_mod, modifier) in MODIFIER_MAP {
        if key_from_code == *gdk_mod {
            result |= *modifier;
        }
    }
    result
}

pub(crate) fn make_key_event(
    keyval_without_modifiers: RawKey,
    scancode: u16,
    is_repeat: bool,
    key_override: Option<KeyCode>,
    state: ElementState,
) -> Option<KeyEvent> {
    let keyval_with_modifiers =
        hardware_keycode_to_keyval(scancode).unwrap_or(keyval_without_modifiers);
    // get unicode value, with and without modifiers
    let text_without_modifiers = keyval_with_modifiers.to_unicode();
    let text_with_modifiers = keyval_without_modifiers.to_unicode();
    // get physical key from the scancode (keycode)
    let physical_key = key_override.unwrap_or_else(|| KeyCode::from_scancode(scancode as u32));

    // extract key without modifier
    let key_without_modifiers = raw_key_to_key(keyval_with_modifiers).unwrap_or_else(|| {
        if let Some(key) = text_without_modifiers {
            if key >= ' ' && key != '\x7f' {
                Key::Character(insert_or_get_key_str(key.to_string()))
            } else {
                Key::Unidentified(NativeKeyCode::Gtk(scancode))
            }
        } else {
            Key::Unidentified(NativeKeyCode::Gtk(scancode))
        }
    });

    // extract the logical key
    let logical_key = raw_key_to_key(keyval_without_modifiers).unwrap_or_else(|| {
        if let Some(key) = text_with_modifiers {
            if key >= ' ' && key != '\x7f' {
                Key::Character(insert_or_get_key_str(key.to_string()))
            } else {
                Key::Unidentified(NativeKeyCode::Gtk(scancode))
            }
        } else {
            Key::Unidentified(NativeKeyCode::Gtk(scancode))
        }
    });

    // make sure we have a valid key
    if !matches!(key_without_modifiers, Key::Unidentified(_)) {
        let location = raw_key_to_location(keyval_with_modifiers);
        let text_with_all_modifiers =
            text_without_modifiers.map(|text| insert_or_get_key_str(text.to_string()));
        return Some(KeyEvent {
            location,
            logical_key,
            physical_key,
            repeat: is_repeat,
            state,
            text: text_with_all_modifiers,
            platform_specific: KeyEventExtra {
                text_with_all_modifiers,
                key_without_modifiers,
            },
        });
    } else {
        #[cfg(debug_assertions)]
        eprintln!("Couldn't get key from code: {physical_key:?}");
    }
    None
}

/// Map a hardware keycode to a keyval by performing a lookup in the keymap and finding the
/// keyval with the lowest group and level
fn hardware_keycode_to_keyval(keycode: u16) -> Option<RawKey> {
    let display = gdk::Display::default()?;
    display
        .map_keycode(keycode as u32)?
        .into_iter()
        .min_by_key(|(key, _)| (key.group(), key.level()))
        .map(|(_, keyval)| keyval)
}
