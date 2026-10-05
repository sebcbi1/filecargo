//! gpui key events → the UI-neutral keys of `filecargo-terminal` (which encodes them).

use filecargo_app_core::prelude::{Key, Mods};
use gpui_kit::Modifiers;

/// `key` is gpui's name of the key (`"a"`, `"enter"`, `"pageup"`, `"f5"`, `"space"`), `key_char`
/// the text it would type. `None` for keys a terminal cannot send.
pub fn map_keystroke(
    key: &str,
    key_char: Option<&str>,
    modifiers: Modifiers,
) -> Option<(Key, Mods)> {
    let mods = Mods {
        ctrl: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
    };
    let named = match key {
        "enter" => Some(Key::Enter),
        "tab" if modifiers.shift => Some(Key::BackTab),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "escape" => Some(Key::Escape),
        "delete" => Some(Key::Delete),
        "insert" => Some(Key::Insert),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "space" => Some(Key::Char(' ')),
        _ => None,
    };
    if let Some(named) = named {
        // BackTab already carries the shift
        let mods = if named == Key::BackTab {
            Mods {
                shift: false,
                ..mods
            }
        } else {
            mods
        };
        return Some((named, mods));
    }
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&number)
    {
        return Some((Key::F(number), mods));
    }
    let mut chars = key.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    };
    // typed text wins (it carries the shift and the keyboard layout), except with ctrl / alt,
    // where the terminal wants the base key plus the modifier
    if !modifiers.control
        && !modifiers.alt
        && let Some(text) = key_char
    {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Some((
                Key::Char(c),
                Mods {
                    shift: false,
                    ..mods
                },
            ));
        }
    }
    single.map(|c| (Key::Char(c), mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn named_keys_and_function_keys() {
        assert_eq!(
            map_keystroke("enter", Some("\n"), mods()),
            Some((Key::Enter, Mods::NONE))
        );
        assert_eq!(
            map_keystroke("up", None, mods()),
            Some((Key::Up, Mods::NONE))
        );
        assert_eq!(
            map_keystroke("pagedown", None, mods()),
            Some((Key::PageDown, Mods::NONE))
        );
        assert_eq!(
            map_keystroke("f5", None, mods()),
            Some((Key::F(5), Mods::NONE))
        );
        assert_eq!(map_keystroke("f13", None, mods()), None);
        assert_eq!(
            map_keystroke("space", Some(" "), mods()),
            Some((Key::Char(' '), Mods::NONE))
        );
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(
            map_keystroke("tab", None, shift),
            Some((Key::BackTab, Mods::NONE))
        );
    }

    #[test]
    fn typed_text_wins_and_keeps_its_case_and_layout() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(
            map_keystroke("a", Some("A"), shift),
            Some((Key::Char('A'), Mods::NONE))
        );
        assert_eq!(
            map_keystroke("1", Some("é"), mods()),
            Some((Key::Char('é'), Mods::NONE))
        );
    }

    #[test]
    fn ctrl_and_alt_send_the_base_key_with_the_modifier() {
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            map_keystroke("c", Some("c"), ctrl),
            Some((Key::Char('c'), Mods::CTRL))
        );
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(
            map_keystroke("b", Some("∫"), alt),
            Some((Key::Char('b'), Mods::ALT))
        );
    }

    #[test]
    fn modifier_only_and_unknown_keys_are_dropped() {
        assert_eq!(map_keystroke("shift", None, mods()), None);
        assert_eq!(map_keystroke("capslock", None, mods()), None);
    }

    #[test]
    fn the_encoded_bytes_are_what_xterm_sends() {
        use filecargo_app_core::prelude::{Modes, encode};
        let (key, m) = map_keystroke(
            "c",
            Some("c"),
            Modifiers {
                control: true,
                ..mods()
            },
        )
        .unwrap();
        assert_eq!(encode(key, m, Modes::default()), [0x03]);
        let (key, m) = map_keystroke("up", None, mods()).unwrap();
        assert_eq!(encode(key, m, Modes::default()), b"\x1b[A");
    }
}
