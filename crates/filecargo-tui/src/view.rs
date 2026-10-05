//! Pure drawing: `(UiState, AppState) -> Frame`.

use filecargo_app_core::prelude::*;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::layout;
use crate::pane::{PaneView, abbreviate, format_size, format_time, pane_view};
use crate::ui_state::{Focus, PaneUi, UiState};

/// Colors only when allowed; emphasis otherwise comes from bold / reverse / markers.
struct Look {
    color: bool,
}

impl Look {
    fn tint(&self, color: Color) -> Style {
        if self.color {
            Style::new().fg(color)
        } else {
            Style::new()
        }
    }

    fn border(&self, focused: bool) -> Style {
        match (focused, self.color) {
            (true, true) => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            (true, false) => Style::new().add_modifier(Modifier::BOLD),
            (false, _) => Style::new(),
        }
    }
}

pub fn render(frame: &mut Frame, ui: &UiState, app: &AppState) {
    let area = frame.area();
    if layout::too_small(area.width, area.height) {
        let message = Paragraph::new(format!(
            "terminal too small (needs {}x{})",
            layout::MIN_WIDTH,
            layout::MIN_HEIGHT
        ))
        .alignment(Alignment::Center);
        let middle = Rect {
            y: area.y + area.height / 2,
            height: 1,
            ..area
        };
        frame.render_widget(message, middle);
        return;
    }
    let look = Look { color: ui.color };
    let areas = layout::areas(area, ui.tree_visible(), ui.maximize_bottom);
    if let Some(tree) = areas.tree {
        placeholder(frame, tree, " Servers ", ui.focus == Focus::Tree, &look);
    }
    if !ui.maximize_bottom {
        render_pane(frame, areas.local, Focus::Local, ui, app, &look);
        render_pane(frame, areas.remote, Focus::Remote, ui, app, &look);
    }
    placeholder(
        frame,
        areas.bottom,
        " Queue ",
        ui.focus == Focus::Bottom,
        &look,
    );
    frame.render_widget(Paragraph::new(" q quit  Tab focus"), areas.status);
}

fn placeholder(frame: &mut Frame, area: Rect, title: &str, focused: bool, look: &Look) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(look.border(focused));
    frame.render_widget(block, area);
}

fn render_pane(
    frame: &mut Frame,
    area: Rect,
    focus: Focus,
    ui: &UiState,
    app: &AppState,
    look: &Look,
) {
    let focused = ui.focus == focus;
    let Some(view) = pane_view(app, focus) else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Remote (not connected) ")
            .border_style(look.border(focused));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("Select a server and press Enter.").alignment(Alignment::Center),
            Rect {
                y: inner.y + inner.height / 2,
                height: 1,
                ..inner
            },
        );
        return;
    };
    let ui_pane = ui.pane(focus).cloned().unwrap_or_default();
    let label = match focus {
        Focus::Local => "Local",
        _ => "Remote",
    };
    let shown = if focus == Focus::Local {
        abbreviate(&view.path, ui.home.as_deref())
    } else {
        view.path.clone()
    };
    let mut title = format!(" {label} {shown} ");
    if view.loading {
        title.push_str("… ");
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(look.border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows_visible = inner.height.saturating_sub(1) as usize; // minus the header
    let table = pane_table(&view, &ui_pane, ui, look, focused, rows_visible);
    let table_area = Rect {
        height: inner.height.saturating_sub(u16::from(view.error.is_some())),
        ..inner
    };
    frame.render_widget(table, table_area);
    if let Some(error) = view.error {
        let line = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled(error.to_owned(), look.tint(Color::Red))),
            line,
        );
    }
}

fn pane_table<'a>(
    view: &PaneView<'a>,
    ui_pane: &PaneUi,
    ui: &UiState,
    look: &Look,
    focused: bool,
    rows_visible: usize,
) -> Table<'a> {
    let header =
        Row::new(["", "Name", "Size", "Modified"]).style(Style::new().add_modifier(Modifier::BOLD));
    let mut rows = Vec::new();
    for row in ui_pane.offset..(ui_pane.offset + rows_visible).min(view.rows()) {
        let entry = view.entry(row);
        let (name, size, modified, is_dir, selected) = match entry {
            None => ("..".to_owned(), String::new(), String::new(), true, false),
            Some(e) => {
                let name = match &e.kind {
                    EntryKind::Dir => format!("{}/", e.name),
                    EntryKind::Symlink { .. } => format!("{}@", e.name),
                    _ => e.name.clone(),
                };
                let size = if e.is_dir() {
                    String::new()
                } else {
                    format_size(e.size)
                };
                (
                    name,
                    size,
                    format_time(e.modified, ui.now),
                    e.is_dir(),
                    ui_pane.selected.contains(&e.name),
                )
            }
        };
        let mut style = Style::new();
        if is_dir {
            style = style
                .patch(look.tint(Color::Blue))
                .add_modifier(Modifier::BOLD);
        }
        if selected {
            style = style
                .patch(look.tint(Color::Yellow))
                .add_modifier(Modifier::BOLD);
        }
        if row == ui_pane.cursor {
            style = style.add_modifier(if focused {
                Modifier::REVERSED
            } else {
                Modifier::UNDERLINED
            });
        }
        rows.push(
            Row::new([
                Cell::from(if selected { "*" } else { " " }),
                Cell::from(name),
                Cell::from(Line::from(size).alignment(Alignment::Right)),
                Cell::from(modified),
            ])
            .style(style),
        );
    }
    Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(11),
        ],
    )
    .header(header)
}
