//! Pure key handling: `(UiState, AppState, Event) -> Commands`. No I/O, no terminal.

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};

use crate::ui_state::UiState;

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Paste(String),
}

/// Keeps `ui` consistent with a new snapshot (cursors in range, selections pruned).
pub fn sync(_ui: &mut UiState, _app: &AppState) {}

pub fn on_event(ui: &mut UiState, _app: &AppState, event: Event) -> Vec<Command> {
    match event {
        Event::Resize(width, height) => {
            ui.size = (width, height);
            Vec::new()
        }
        Event::Key(KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            ..
        })
        | Event::Key(KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }) => {
            ui.quit_requested = true;
            vec![Command::Quit]
        }
        _ => Vec::new(),
    }
}
