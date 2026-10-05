//! UI-neutral key events and the bytes a terminal sends for them (xterm conventions).

/// A key press, independent of any UI toolkit. Each front-end maps its own key type to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Escape,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 ..= F12
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
    };
    pub const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
    };
    pub const ALT: Mods = Mods {
        ctrl: false,
        alt: true,
        shift: false,
    };
    pub const SHIFT: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: true,
    };

    /// xterm's modifier parameter: 1 + shift (1) + alt (2) + ctrl (4); 1 means none.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }

    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// Terminal modes the remote application can switch, read from the screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: arrows, Home and End send `SS3` instead of `CSI`.
    pub application_cursor: bool,
    /// DECKPAM. No key of [`Key`] is a keypad key in v1, so this has no effect yet.
    pub application_keypad: bool,
}

fn csi(final_byte: char) -> Vec<u8> {
    format!("\x1b[{final_byte}").into_bytes()
}

fn ss3(final_byte: char) -> Vec<u8> {
    format!("\x1bO{final_byte}").into_bytes()
}

/// `CSI 1 ; <mods> <final>` for the letter-terminated keys.
fn modified_letter(final_byte: char, mods: Mods) -> Vec<u8> {
    format!("\x1b[1;{}{final_byte}", mods.parameter()).into_bytes()
}

/// `CSI <number> [; <mods>] ~` for the tilde-terminated keys.
fn tilde(number: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{number};{}~", mods.parameter()).into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn cursor_key(final_byte: char, mods: Mods, modes: Modes) -> Vec<u8> {
    if mods.any() {
        modified_letter(final_byte, mods)
    } else if modes.application_cursor {
        ss3(final_byte)
    } else {
        csi(final_byte)
    }
}

/// The C0 control byte for Ctrl + `c`, when there is one.
fn control_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// With Alt, the sequence is prefixed by ESC.
fn alt_prefixed(mods: Mods, mut bytes: Vec<u8>) -> Vec<u8> {
    if mods.alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// The bytes a terminal sends for `key`, following xterm. Pure: the caller supplies the
/// modes the remote application has set.
pub fn encode(key: Key, mods: Mods, modes: Modes) -> Vec<u8> {
    match key {
        Key::Char(c) => {
            let bytes = if mods.ctrl {
                control_byte(c).map_or_else(|| c.to_string().into_bytes(), |b| vec![b])
            } else {
                c.to_string().into_bytes()
            };
            alt_prefixed(mods, bytes)
        }
        Key::Enter => alt_prefixed(mods, vec![b'\r']),
        Key::Tab if mods.shift => csi('Z'),
        Key::Tab => alt_prefixed(mods, vec![b'\t']),
        Key::BackTab => csi('Z'),
        Key::Backspace => alt_prefixed(mods, vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Escape => alt_prefixed(mods, vec![0x1b]),
        Key::Up => cursor_key('A', mods, modes),
        Key::Down => cursor_key('B', mods, modes),
        Key::Right => cursor_key('C', mods, modes),
        Key::Left => cursor_key('D', mods, modes),
        Key::Home => cursor_key('H', mods, modes),
        Key::End => cursor_key('F', mods, modes),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => {
            let letter = ['P', 'Q', 'R', 'S'][usize::from(n) - 1];
            if mods.any() {
                modified_letter(letter, mods)
            } else {
                ss3(letter)
            }
        }
        Key::F(n @ 5..=12) => {
            let number = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n) - 5];
            tilde(number, mods)
        }
        Key::F(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Modes = Modes {
        application_cursor: false,
        application_keypad: false,
    };
    const APP: Modes = Modes {
        application_cursor: true,
        application_keypad: false,
    };

    fn s(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap().replace('\x1b', "ESC")
    }

    /// (key, mods, normal-mode expectation, application-cursor-mode expectation)
    #[test]
    fn every_key_in_both_cursor_modes_against_xterms_sequences() {
        use Key::*;
        let n = Mods::NONE;
        let (c, a, sh) = (Mods::CTRL, Mods::ALT, Mods::SHIFT);
        #[rustfmt::skip]
        let table: &[(Key, Mods, &str, &str)] = &[
            // text and C0 controls
            (Char('a'), n, "a", "a"),
            (Char('é'), n, "é", "é"),
            (Char('A'), sh, "A", "A"),
            (Char('a'), c, "\u{1}", "\u{1}"),
            (Char('z'), c, "\u{1a}", "\u{1a}"),
            (Char('['), c, "ESC", "ESC"),
            (Char('@'), c, "\0", "\0"),
            (Char('?'), c, "\u{7f}", "\u{7f}"),
            (Char('5'), c, "5", "5"),
            (Char('a'), a, "ESCa", "ESCa"),
            (Char('a'), Mods { ctrl: true, alt: true, shift: false }, "ESC\u{1}", "ESC\u{1}"),
            (Enter, n, "\r", "\r"),
            (Enter, a, "ESC\r", "ESC\r"),
            (Enter, c, "\r", "\r"),
            (Enter, sh, "\r", "\r"),
            (Tab, n, "\t", "\t"),
            (Tab, sh, "ESC[Z", "ESC[Z"),
            (Tab, a, "ESC\t", "ESC\t"),
            (Tab, c, "\t", "\t"),
            (BackTab, n, "ESC[Z", "ESC[Z"),
            (Backspace, n, "\u{7f}", "\u{7f}"),
            (Backspace, c, "\u{8}", "\u{8}"),
            (Backspace, a, "ESC\u{7f}", "ESC\u{7f}"),
            (Backspace, sh, "\u{7f}", "\u{7f}"),
            (Escape, n, "ESC", "ESC"),
            (Escape, a, "ESCESC", "ESCESC"),
            // cursor keys: SS3 in application mode, modified forms ignore the mode
            (Up, n, "ESC[A", "ESCOA"),
            (Down, n, "ESC[B", "ESCOB"),
            (Right, n, "ESC[C", "ESCOC"),
            (Left, n, "ESC[D", "ESCOD"),
            (Home, n, "ESC[H", "ESCOH"),
            (End, n, "ESC[F", "ESCOF"),
            (Up, sh, "ESC[1;2A", "ESC[1;2A"),
            (Up, a, "ESC[1;3A", "ESC[1;3A"),
            (Up, c, "ESC[1;5A", "ESC[1;5A"),
            (Left, Mods { ctrl: true, alt: false, shift: true }, "ESC[1;6D", "ESC[1;6D"),
            (Right, Mods { ctrl: true, alt: true, shift: true }, "ESC[1;8C", "ESC[1;8C"),
            (Home, c, "ESC[1;5H", "ESC[1;5H"),
            (End, sh, "ESC[1;2F", "ESC[1;2F"),
            // editing keys
            (Insert, n, "ESC[2~", "ESC[2~"),
            (Delete, n, "ESC[3~", "ESC[3~"),
            (PageUp, n, "ESC[5~", "ESC[5~"),
            (PageDown, n, "ESC[6~", "ESC[6~"),
            (Delete, c, "ESC[3;5~", "ESC[3;5~"),
            (PageUp, sh, "ESC[5;2~", "ESC[5;2~"),
            (Insert, a, "ESC[2;3~", "ESC[2;3~"),
            // function keys
            (F(1), n, "ESCOP", "ESCOP"),
            (F(2), n, "ESCOQ", "ESCOQ"),
            (F(3), n, "ESCOR", "ESCOR"),
            (F(4), n, "ESCOS", "ESCOS"),
            (F(5), n, "ESC[15~", "ESC[15~"),
            (F(6), n, "ESC[17~", "ESC[17~"),
            (F(7), n, "ESC[18~", "ESC[18~"),
            (F(8), n, "ESC[19~", "ESC[19~"),
            (F(9), n, "ESC[20~", "ESC[20~"),
            (F(10), n, "ESC[21~", "ESC[21~"),
            (F(11), n, "ESC[23~", "ESC[23~"),
            (F(12), n, "ESC[24~", "ESC[24~"),
            (F(1), sh, "ESC[1;2P", "ESC[1;2P"),
            (F(4), c, "ESC[1;5S", "ESC[1;5S"),
            (F(5), a, "ESC[15;3~", "ESC[15;3~"),
            (F(12), Mods { ctrl: true, alt: false, shift: true }, "ESC[24;6~", "ESC[24;6~"),
            // out of range
            (F(0), n, "", ""),
            (F(13), n, "", ""),
        ];
        for (key, mods, normal, application) in table {
            assert_eq!(
                s(encode(*key, *mods, PLAIN)),
                *normal,
                "{key:?} {mods:?} (normal)"
            );
            assert_eq!(
                s(encode(*key, *mods, APP)),
                *application,
                "{key:?} {mods:?} (application)"
            );
        }
    }

    #[test]
    fn every_key_variant_is_covered_by_the_table_above() {
        // a new variant must be added to the table: this match stops compiling until it is
        fn covered(key: Key) -> bool {
            match key {
                Key::Char(_)
                | Key::Enter
                | Key::Tab
                | Key::BackTab
                | Key::Backspace
                | Key::Escape
                | Key::Delete
                | Key::Insert
                | Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::F(_) => true,
            }
        }
        assert!(covered(Key::Up));
    }
}
