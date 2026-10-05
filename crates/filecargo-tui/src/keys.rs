//! crossterm key events → the UI-neutral keys of `filecargo-terminal` (which encodes them).

use filecargo_app_core::prelude::{Key, Mods};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `None` for keys a terminal cannot send (media keys, bare modifiers, ...).
pub fn to_terminal_key(event: KeyEvent) -> Option<(Key, Mods)> {
    let key = match event.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Escape,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n @ 1..=12) => Key::F(n),
        _ => return None,
    };
    let mods = Mods {
        ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
        alt: event.modifiers.contains(KeyModifiers::ALT),
        shift: event.modifiers.contains(KeyModifiers::SHIFT),
    };
    Some((key, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(code: KeyCode, modifiers: KeyModifiers) -> Option<(Key, Mods)> {
        to_terminal_key(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn letters_function_keys_and_modifiers_map_through() {
        assert_eq!(
            map(KeyCode::Char('a'), KeyModifiers::NONE),
            Some((Key::Char('a'), Mods::NONE))
        );
        assert_eq!(
            map(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some((Key::Char('c'), Mods::CTRL))
        );
        assert_eq!(
            map(KeyCode::Char('b'), KeyModifiers::ALT),
            Some((Key::Char('b'), Mods::ALT))
        );
        assert_eq!(
            map(KeyCode::Up, KeyModifiers::SHIFT),
            Some((Key::Up, Mods::SHIFT))
        );
        assert_eq!(
            map(KeyCode::F(5), KeyModifiers::NONE),
            Some((Key::F(5), Mods::NONE))
        );
        assert_eq!(
            map(KeyCode::Esc, KeyModifiers::NONE),
            Some((Key::Escape, Mods::NONE))
        );
        assert_eq!(
            map(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some((Key::BackTab, Mods::SHIFT))
        );
    }

    #[test]
    fn keys_a_terminal_cannot_send_are_dropped() {
        assert_eq!(map(KeyCode::F(13), KeyModifiers::NONE), None);
        assert_eq!(map(KeyCode::CapsLock, KeyModifiers::NONE), None);
        assert_eq!(map(KeyCode::Null, KeyModifiers::NONE), None);
    }

    #[test]
    fn the_mapped_keys_encode_as_xterm_expects() {
        use filecargo_app_core::prelude::{Modes, encode};
        let (key, mods) = map(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();
        assert_eq!(encode(key, mods, Modes::default()), [0x03]);
        let (key, mods) = map(KeyCode::Up, KeyModifiers::NONE).unwrap();
        assert_eq!(encode(key, mods, Modes::default()), b"\x1b[A");
    }
}
