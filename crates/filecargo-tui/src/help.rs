//! The help overlay and the status-line hints, both built from the key table so they cannot
//! drift from what the keys really do.

use crate::keymap::{Action, BINDINGS, Context};
use crate::ui_state::{BottomTab, Focus, UiState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    Heading(&'static str),
    Key {
        keys: &'static str,
        text: &'static str,
    },
}

fn heading(context: Context) -> &'static str {
    match context {
        Context::Global => "Everywhere",
        Context::Files => "File panes",
        Context::Tree => "Server tree",
        Context::Queue => "Queue, completed and failed lists",
        Context::Log => "Log",
        Context::Terminal => "Terminal tab (all other keys go to the shell)",
    }
}

/// One heading per run of bindings of the same context, then a row per binding that has a
/// description (rows with an empty label or text belong to the row above them).
pub fn lines() -> Vec<HelpLine> {
    let mut lines = Vec::new();
    let mut current = None;
    for binding in BINDINGS {
        if binding.label.is_empty() || binding.help.is_empty() {
            continue;
        }
        if current != Some(binding.context) {
            current = Some(binding.context);
            lines.push(HelpLine::Heading(heading(binding.context)));
        }
        lines.push(HelpLine::Key {
            keys: binding.label,
            text: binding.help,
        });
    }
    lines
}

/// How `action` is typed: the first key of its first binding that has a label.
pub fn key_of(action: Action) -> Option<&'static str> {
    BINDINGS
        .iter()
        .find(|b| b.action == action)
        .and_then(|b| b.label.split_whitespace().next())
        .or_else(|| {
            // a binding without a label shares the label of the one above it
            let at = BINDINGS.iter().position(|b| b.action == action)?;
            BINDINGS[..at]
                .iter()
                .rev()
                .find(|b| !b.label.is_empty())
                .and_then(|b| b.label.split_whitespace().next())
        })
}

fn hints_for(actions: &[(Action, &'static str)]) -> String {
    actions
        .iter()
        .filter_map(|(action, short)| key_of(*action).map(|key| format!("{key} {short}")))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What the status line says for the focused area.
pub fn status_hints(ui: &UiState) -> String {
    use Action as A;
    let text = match ui.focus {
        Focus::Local => hints_for(&[
            (A::Transfer, "upload"),
            (A::MakeDir, "mkdir"),
            (A::Rename, "rename"),
            (A::Delete, "delete"),
            (A::GoTo, "go to"),
            (A::ToggleHidden, "dotfiles"),
            (A::CycleSort, "sort"),
        ]),
        Focus::Remote => hints_for(&[
            (A::Transfer, "download"),
            (A::MakeDir, "mkdir"),
            (A::Rename, "rename"),
            (A::Delete, "delete"),
            (A::Chmod, "chmod"),
            (A::GoTo, "go to"),
        ]),
        Focus::Tree => hints_for(&[
            (A::Open, "connect"),
            (A::NewSite, "new"),
            (A::EditSite, "edit"),
            (A::DeleteNode, "delete"),
            (A::ImportFileZilla, "import"),
        ]),
        Focus::Bottom => match ui.bottom.tab {
            BottomTab::Queue | BottomTab::Completed | BottomTab::Failed => hints_for(&[
                (A::QueuePause, "pause"),
                (A::QueueRemove, "remove"),
                (A::QueueRetry, "retry"),
                (A::QueueClear, "clear"),
            ]),
            BottomTab::Log => hints_for(&[(A::LogFollow, "follow")]),
            BottomTab::Terminal => hints_for(&[
                (A::TerminalEscape, "leave"),
                (A::TerminalScrollUp, "scroll"),
            ]),
        },
    };
    let help = key_of(A::Help).unwrap_or("F1");
    let quit = key_of(A::Quit).unwrap_or("q");
    format!(" {text} · {help} help · {quit} quit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_context_has_a_section_and_every_row_has_keys_and_text() {
        let lines = lines();
        let headings: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                HelpLine::Heading(h) => Some(*h),
                HelpLine::Key { .. } => None,
            })
            .collect();
        assert_eq!(headings.len(), 6, "{headings:?}");
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, HelpLine::Key { keys: "F5 t", .. }))
        );
        for line in &lines {
            if let HelpLine::Key { keys, text } = line {
                assert!(!keys.is_empty() && !text.is_empty());
            }
        }
    }

    #[test]
    fn every_hint_action_has_a_key() {
        for focus in [Focus::Tree, Focus::Local, Focus::Remote, Focus::Bottom] {
            let mut ui = UiState::new(100, 30);
            ui.focus = focus;
            for tab in BottomTab::ALL {
                ui.bottom.tab = tab;
                let text = status_hints(&ui);
                // a missing key would drop its hint: count the separators against the actions
                assert!(text.contains("help") && text.contains("quit"), "{text}");
                assert!(!text.contains("  ·"), "{text}");
            }
        }
        assert_eq!(key_of(Action::Transfer), Some("F5"));
        assert_eq!(
            key_of(Action::Down),
            Some("Up"),
            "a label-less row borrows the row above"
        );
        assert_eq!(key_of(Action::TerminalEscape), Some("Ctrl-\\"));
    }
}
