//! The utility dialogs: change permissions (octal and rwx kept in step) and FileZilla import.

use std::path::PathBuf;

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::KeyEvent;

use crate::dialog::Outcome;
use crate::form::{Field, Form, FormOutcome};

const BITS: [(&str, &str, u32); 9] = [
    ("ur", "Owner read", 0o400),
    ("uw", "Owner write", 0o200),
    ("ux", "Owner execute", 0o100),
    ("gr", "Group read", 0o040),
    ("gw", "Group write", 0o020),
    ("gx", "Group execute", 0o010),
    ("or", "Others read", 0o004),
    ("ow", "Others write", 0o002),
    ("ox", "Others execute", 0o001),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChmodDialog {
    pub form: Form,
    pub names: Vec<String>,
}

impl ChmodDialog {
    pub fn new(names: Vec<String>, mode: u32) -> Self {
        let mut fields = vec![Field::text(
            "octal",
            "Octal",
            &format!("{:o}", mode & 0o777),
        )];
        fields.extend(
            BITS.iter()
                .map(|(id, label, bit)| Field::checkbox(id, label, mode & bit != 0)),
        );
        Self {
            form: Form::new(fields),
            names,
        }
    }

    /// The mode shown in the form, `None` while the octal text is not a valid mode.
    pub fn mode(&self) -> Option<u32> {
        let text = self.form.text_of("octal");
        if text.is_empty() || text.len() > 4 {
            return None;
        }
        u32::from_str_radix(text, 8).ok().filter(|m| *m <= 0o7777)
    }

    fn bits(&self) -> u32 {
        BITS.iter()
            .filter(|(id, _, _)| self.form.checked(id))
            .fold(0, |m, (_, _, bit)| m | bit)
    }

    /// Keeps the two views of the mode in step: typing octal sets the boxes, toggling a box
    /// rewrites the octal text (keeping any special bits typed above the 9 permission bits).
    fn sync(&mut self) {
        let on_octal = self
            .form
            .fields
            .get(self.form.focus)
            .is_some_and(|f| f.id == "octal");
        if on_octal {
            if let Some(mode) = self.mode() {
                for (id, _, bit) in BITS {
                    if let Some(field) = self.form.field_mut(id) {
                        field.checked = mode & bit != 0;
                    }
                }
            }
        } else {
            let special = self.mode().map_or(0, |m| m & 0o7000);
            let text = format!("{:o}", special | self.bits());
            if let Some(field) = self.form.field_mut("octal") {
                field.set_text(&text);
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match self.form.on_key(key) {
            FormOutcome::Cancel => Outcome::Close,
            FormOutcome::Handled => {
                self.sync();
                Outcome::Keep
            }
            FormOutcome::Submit => match self.mode() {
                Some(mode) => Outcome::Run(vec![Command::Chmod {
                    pane: PaneId::Remote,
                    names: self.names.clone(),
                    mode,
                }]),
                None => {
                    self.form.error = Some("Enter the mode as 3 or 4 octal digits.".to_owned());
                    Outcome::Keep
                }
            },
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.form.paste(text);
        self.sync();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDialog {
    pub form: Form,
}

impl ImportDialog {
    pub fn new(default_path: &str) -> Self {
        Self {
            form: Form::new(vec![
                Field::text("path", "sitemanager.xml", default_path),
                Field::checkbox("passwords", "Import passwords", false),
            ]),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match self.form.on_key(key) {
            FormOutcome::Cancel => Outcome::Close,
            FormOutcome::Handled => Outcome::Keep,
            FormOutcome::Submit => {
                let path = self.form.text_of("path");
                if path.is_empty() {
                    self.form.error = Some("Enter the path of the file.".to_owned());
                    return Outcome::Keep;
                }
                Outcome::Run(vec![Command::ImportFileZilla {
                    path: PathBuf::from(path),
                    import_passwords: self.form.checked("passwords"),
                }])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn checked(dialog: &ChmodDialog, id: &str) -> bool {
        dialog.form.checked(id)
    }

    #[test]
    fn the_boxes_follow_the_initial_mode() {
        let dialog = ChmodDialog::new(vec!["a".into()], 0o640);
        assert_eq!(dialog.form.text_of("octal"), "640");
        assert!(checked(&dialog, "ur") && checked(&dialog, "uw") && !checked(&dialog, "ux"));
        assert!(checked(&dialog, "gr") && !checked(&dialog, "gw"));
        assert!(!checked(&dialog, "or") && !checked(&dialog, "ow") && !checked(&dialog, "ox"));
    }

    #[test]
    fn typing_octal_updates_the_boxes_and_a_box_updates_the_octal() {
        let mut dialog = ChmodDialog::new(vec!["a".into()], 0o644);
        dialog.on_key(key(KeyCode::Char('x'))); // invalid: ignored by the boxes
        assert_eq!(dialog.mode(), None);
        dialog.on_key(key(KeyCode::Backspace));
        dialog.on_key(key(KeyCode::Backspace));
        dialog.on_key(key(KeyCode::Backspace));
        dialog.on_key(key(KeyCode::Backspace));
        for c in "755".chars() {
            dialog.on_key(key(KeyCode::Char(c)));
        }
        assert!(checked(&dialog, "ux") && checked(&dialog, "gx") && checked(&dialog, "ox"));
        assert!(!checked(&dialog, "gw"));
        // move to "Group write" (octal, ur, uw, ux, gr, gw) and tick it
        for _ in 0..5 {
            dialog.on_key(key(KeyCode::Tab));
        }
        dialog.on_key(key(KeyCode::Char(' ')));
        assert_eq!(dialog.form.text_of("octal"), "775");
        assert_eq!(dialog.mode(), Some(0o775));
    }

    #[test]
    fn enter_runs_chmod_on_the_names_and_a_bad_octal_is_refused() {
        let mut dialog = ChmodDialog::new(vec!["a".into(), "b".into()], 0o600);
        let Outcome::Run(commands) = dialog.on_key(key(KeyCode::Enter)) else {
            panic!()
        };
        assert!(
            matches!(&commands[..], [Command::Chmod { names, mode: 0o600, .. }] if names.len() == 2)
        );
        dialog.on_key(key(KeyCode::Char('9')));
        assert!(matches!(dialog.on_key(key(KeyCode::Enter)), Outcome::Keep));
        assert_eq!(
            dialog.form.error.as_deref(),
            Some("Enter the mode as 3 or 4 octal digits.")
        );
        assert!(matches!(dialog.on_key(key(KeyCode::Esc)), Outcome::Close));
    }

    #[test]
    fn special_bits_typed_in_octal_survive_a_box_toggle() {
        let mut dialog = ChmodDialog::new(vec!["a".into()], 0o644);
        dialog.on_key(key(KeyCode::Home));
        dialog.on_key(key(KeyCode::Char('1'))); // 1644: sticky
        assert_eq!(dialog.mode(), Some(0o1644));
        dialog.on_key(key(KeyCode::Tab));
        dialog.on_key(key(KeyCode::Tab));
        dialog.on_key(key(KeyCode::Tab));
        dialog.on_key(key(KeyCode::Char(' '))); // owner execute
        assert_eq!(dialog.form.text_of("octal"), "1744");
    }

    #[test]
    fn import_needs_a_path_and_carries_the_password_choice() {
        let mut dialog = ImportDialog::new("");
        assert!(matches!(dialog.on_key(key(KeyCode::Enter)), Outcome::Keep));
        assert_eq!(
            dialog.form.error.as_deref(),
            Some("Enter the path of the file.")
        );
        dialog.form.paste("/tmp/sitemanager.xml");
        dialog.on_key(key(KeyCode::Tab));
        dialog.on_key(key(KeyCode::Char(' ')));
        let Outcome::Run(commands) = dialog.on_key(key(KeyCode::Enter)) else {
            panic!()
        };
        assert!(matches!(
            &commands[..],
            [Command::ImportFileZilla { path, import_passwords: true }] if path == &PathBuf::from("/tmp/sitemanager.xml")
        ));
    }
}
