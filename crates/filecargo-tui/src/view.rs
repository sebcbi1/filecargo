//! Pure drawing: `(UiState, AppState) -> Frame`.

use filecargo_app_core::prelude::*;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Borders};

use crate::ui_state::UiState;

pub fn render(frame: &mut Frame, _ui: &UiState, _app: &AppState) {
    let area = frame.area();
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(area);
    let [tree, local, remote] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(40),
        Constraint::Percentage(40),
    ])
    .areas(top);
    for (area, title) in [
        (tree, " Servers "),
        (local, " Local "),
        (remote, " Remote "),
        (bottom, " Queue "),
    ] {
        frame.render_widget(Block::default().borders(Borders::ALL).title(title), area);
    }
}
