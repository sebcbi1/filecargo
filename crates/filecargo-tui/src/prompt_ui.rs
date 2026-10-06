//! Answering the app's prompts: what the keys do for each `PromptKind`. Pure; the reducer sends
//! the answer back as `Command::Answer`.

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::form::{Field, Form, FormOutcome};

/// The UI side of the prompt the app is showing.
#[derive(Debug, Clone)]
pub struct PromptSlot {
    pub id: PromptId,
    pub ui: PromptUi,
    /// The answer went out; the prompt stays in the next snapshot until the app has processed
    /// it, so it is neither drawn nor given keys meanwhile.
    pub answered: bool,
}

#[derive(Debug, Clone)]
pub enum PromptUi {
    /// Masked (or echoed, for keyboard-interactive) fields, plus "remember" where it applies.
    Credential(Form),
    Conflict {
        apply_all: bool,
    },
    Plain,
}

impl PromptUi {
    pub fn for_kind(kind: &PromptKind) -> Self {
        match kind {
            PromptKind::Credential(prompt) => Self::Credential(credential_form(prompt)),
            PromptKind::Conflict { .. } => Self::Conflict { apply_all: false },
            _ => Self::Plain,
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Self::Credential(form) = self {
            form.paste(text);
        }
    }

    /// `Some` when the key answers the prompt.
    pub fn on_key(&mut self, kind: &PromptKind, key: KeyEvent) -> Option<PromptAnswer> {
        match (self, kind) {
            (Self::Credential(form), _) => match form.on_key(key) {
                FormOutcome::Handled => None,
                FormOutcome::Cancel => Some(PromptAnswer::Credential(None)),
                FormOutcome::Submit => {
                    let values = form
                        .fields
                        .iter()
                        .filter(|f| f.id.starts_with('v'))
                        .map(|f| SecretString::from(f.text.clone()))
                        .collect();
                    Some(PromptAnswer::Credential(Some(CredentialAnswer {
                        values,
                        remember: form.checked("remember"),
                    })))
                }
            },
            (Self::Conflict { apply_all }, _) => {
                let rule = match key.code {
                    KeyCode::Char('o') => ConflictRule::Overwrite,
                    KeyCode::Char('n') => ConflictRule::OverwriteIfNewer,
                    KeyCode::Char('r') => ConflictRule::Resume,
                    KeyCode::Char('s') => ConflictRule::Skip,
                    KeyCode::Char('k') => ConflictRule::Rename,
                    KeyCode::Char('a') => {
                        *apply_all = !*apply_all;
                        return None;
                    }
                    KeyCode::Esc => return Some(PromptAnswer::Dismiss),
                    _ => return None,
                };
                Some(PromptAnswer::Conflict(ConflictDecision {
                    rule,
                    apply_to_all: *apply_all,
                }))
            }
            (Self::Plain, PromptKind::HostKey(_) | PromptKind::Certificate(_)) => {
                // no default key: a trust decision is never one stray Enter away
                match key.code {
                    KeyCode::Char('y') => Some(PromptAnswer::Trust(TrustDecision::TrustOnce)),
                    KeyCode::Char('a') => Some(PromptAnswer::Trust(TrustDecision::TrustAlways)),
                    KeyCode::Char('n') | KeyCode::Esc => {
                        Some(PromptAnswer::Trust(TrustDecision::Reject))
                    }
                    _ => None,
                }
            }
            (
                Self::Plain,
                PromptKind::ConfirmDelete { .. }
                | PromptKind::ConfirmQuit { .. }
                | PromptKind::ConfirmClearQueue { .. },
            ) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => Some(PromptAnswer::Confirm(true)),
                KeyCode::Char('n') | KeyCode::Esc => Some(PromptAnswer::Confirm(false)),
                _ => None,
            },
            (Self::Plain, _) => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => Some(PromptAnswer::Dismiss),
                _ => None,
            },
        }
    }
}

fn credential_form(prompt: &CredentialPrompt) -> Form {
    const IDS: [&str; 16] = [
        "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13",
        "v14", "v15",
    ];
    let mut fields = Vec::new();
    match prompt {
        CredentialPrompt::Password { .. } => fields.push(Field::masked("v0", "Password")),
        CredentialPrompt::Passphrase { .. } => fields.push(Field::masked("v0", "Passphrase")),
        CredentialPrompt::KeyboardInteractive { prompts, .. } => {
            for (i, (text, echo)) in prompts.iter().enumerate().take(IDS.len()) {
                let label = text.trim().trim_end_matches(':');
                fields.push(if *echo {
                    Field::text(IDS[i], label, "")
                } else {
                    Field::masked(IDS[i], label)
                });
            }
        }
    }
    if !matches!(prompt, CredentialPrompt::KeyboardInteractive { .. }) {
        fields.push(Field::checkbox("remember", "Remember", false));
    }
    Form::new(fields)
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn answer(kind: &PromptKind, keys: &[KeyCode]) -> Option<PromptAnswer> {
        let mut ui = PromptUi::for_kind(kind);
        let mut last = None;
        for code in keys {
            last = ui.on_key(kind, key(*code));
        }
        last
    }

    fn host_key() -> PromptKind {
        PromptKind::HostKey(HostKeyPrompt {
            host: "example.org".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        })
    }

    #[test]
    fn trust_prompts_have_no_default_key() {
        let kind = host_key();
        assert!(answer(&kind, &[KeyCode::Enter]).is_none());
        assert!(answer(&kind, &[KeyCode::Char(' ')]).is_none());
        assert!(matches!(
            answer(&kind, &[KeyCode::Char('y')]),
            Some(PromptAnswer::Trust(TrustDecision::TrustOnce))
        ));
        assert!(matches!(
            answer(&kind, &[KeyCode::Char('a')]),
            Some(PromptAnswer::Trust(TrustDecision::TrustAlways))
        ));
        assert!(matches!(
            answer(&kind, &[KeyCode::Char('n')]),
            Some(PromptAnswer::Trust(TrustDecision::Reject))
        ));
        assert!(matches!(
            answer(&kind, &[KeyCode::Esc]),
            Some(PromptAnswer::Trust(TrustDecision::Reject))
        ));
    }

    #[test]
    fn a_password_prompt_answers_with_the_typed_value_and_the_remember_choice() {
        let kind = PromptKind::Credential(CredentialPrompt::Password {
            site: "work".into(),
            user: "me".into(),
            retry: false,
        });
        let keys = [
            KeyCode::Char('p'),
            KeyCode::Char('w'),
            KeyCode::Tab,
            KeyCode::Char(' '),
            KeyCode::Enter,
        ];
        let Some(PromptAnswer::Credential(Some(answer))) = answer(&kind, &keys) else {
            panic!()
        };
        assert_eq!(answer.values.len(), 1);
        assert!(answer.remember);
        use filecargo_config::ExposeSecret;
        assert_eq!(answer.values[0].expose_secret(), "pw");
        assert!(matches!(
            self::answer(&kind, &[KeyCode::Esc]),
            Some(PromptAnswer::Credential(None))
        ));
    }

    #[test]
    fn keyboard_interactive_has_one_field_per_prompt_and_no_remember() {
        let kind = PromptKind::Credential(CredentialPrompt::KeyboardInteractive {
            site: "work".into(),
            name: String::new(),
            instructions: String::new(),
            prompts: vec![("Code: ".into(), true), ("PIN: ".into(), false)],
        });
        let PromptUi::Credential(form) = PromptUi::for_kind(&kind) else {
            panic!()
        };
        assert_eq!(form.fields.len(), 2);
        assert_eq!(form.fields[0].label, "Code");
        assert!(matches!(
            form.fields[1].kind,
            crate::form::FieldKind::Masked
        ));
        let keys = [
            KeyCode::Char('1'),
            KeyCode::Tab,
            KeyCode::Char('2'),
            KeyCode::Enter,
        ];
        let Some(PromptAnswer::Credential(Some(answer))) = answer(&kind, &keys) else {
            panic!()
        };
        assert_eq!(answer.values.len(), 2);
        assert!(!answer.remember);
    }

    #[test]
    fn a_passphrase_prompt_is_masked_too() {
        let kind = PromptKind::Credential(CredentialPrompt::Passphrase {
            site: "work".into(),
            key_path: PathBuf::from("/k"),
            retry: true,
        });
        let PromptUi::Credential(form) = PromptUi::for_kind(&kind) else {
            panic!()
        };
        assert!(matches!(
            form.fields[0].kind,
            crate::form::FieldKind::Masked
        ));
    }

    #[test]
    fn conflict_keys_map_to_rules_and_apply_to_all_is_a_toggle() {
        let conflict = ConflictInfo {
            source: Entry::new("a.txt", EntryKind::File),
            target: Entry::new("a.txt", EntryKind::File),
        };
        let kind = PromptKind::Conflict {
            transfer: TransferId(0),
            conflict,
        };
        for (c, rule) in [
            ('o', ConflictRule::Overwrite),
            ('n', ConflictRule::OverwriteIfNewer),
            ('r', ConflictRule::Resume),
            ('s', ConflictRule::Skip),
            ('k', ConflictRule::Rename),
        ] {
            let Some(PromptAnswer::Conflict(d)) = answer(&kind, &[KeyCode::Char(c)]) else {
                panic!("{c}")
            };
            assert_eq!((d.rule, d.apply_to_all), (rule, false), "{c}");
        }
        let Some(PromptAnswer::Conflict(d)) =
            answer(&kind, &[KeyCode::Char('a'), KeyCode::Char('s')])
        else {
            panic!()
        };
        assert!(d.apply_to_all);
        let Some(PromptAnswer::Conflict(d)) = answer(
            &kind,
            &[KeyCode::Char('a'), KeyCode::Char('a'), KeyCode::Char('s')],
        ) else {
            panic!()
        };
        assert!(!d.apply_to_all, "toggled twice");
        assert!(matches!(
            answer(&kind, &[KeyCode::Esc]),
            Some(PromptAnswer::Dismiss)
        ));
        assert!(answer(&kind, &[KeyCode::Enter]).is_none());
    }

    #[test]
    fn confirmations_and_messages() {
        let delete = PromptKind::ConfirmDelete {
            pane: PaneId::Remote,
            names: vec!["a".into()],
            recursive: false,
        };
        assert!(matches!(
            answer(&delete, &[KeyCode::Enter]),
            Some(PromptAnswer::Confirm(true))
        ));
        assert!(matches!(
            answer(&delete, &[KeyCode::Char('y')]),
            Some(PromptAnswer::Confirm(true))
        ));
        assert!(matches!(
            answer(&delete, &[KeyCode::Esc]),
            Some(PromptAnswer::Confirm(false))
        ));
        let quit = PromptKind::ConfirmQuit {
            active_transfers: 2,
        };
        assert!(matches!(
            answer(&quit, &[KeyCode::Char('n')]),
            Some(PromptAnswer::Confirm(false))
        ));
        let clear = PromptKind::ConfirmClearQueue {
            items: 3,
            active: 1,
        };
        assert!(matches!(
            answer(&clear, &[KeyCode::Char('y')]),
            Some(PromptAnswer::Confirm(true))
        ));
        assert!(matches!(
            answer(&clear, &[KeyCode::Esc]),
            Some(PromptAnswer::Confirm(false))
        ));
        let message = PromptKind::Message {
            level: Level::Info,
            title: "t".into(),
            body: "b".into(),
        };
        assert!(answer(&message, &[KeyCode::Char('x')]).is_none());
        assert!(matches!(
            answer(&message, &[KeyCode::Enter]),
            Some(PromptAnswer::Dismiss)
        ));
        assert!(matches!(
            answer(&message, &[KeyCode::Esc]),
            Some(PromptAnswer::Dismiss)
        ));
    }
}
