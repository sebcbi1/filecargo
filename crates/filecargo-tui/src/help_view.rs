//! Drawing of the help overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::dialog_view::centered;
use crate::help::{HelpLine, lines};
use crate::view::Look;

/// Rows of key lines the overlay can show in a terminal of `height` rows.
pub fn visible_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(2).min(40).saturating_sub(4)).max(1)
}

pub fn max_scroll(height: u16) -> usize {
    lines().len().saturating_sub(visible_rows(height))
}

pub(crate) fn render(frame: &mut Frame, area: Rect, scroll: usize, look: &Look) {
    let rect = centered(area, 96, area.height.saturating_sub(2).min(40));
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .border_style(look.border(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let rows = visible_rows(area.height);
    let shown: Vec<Line> = lines()
        .into_iter()
        .skip(scroll)
        .take(rows)
        .map(|line| match line {
            HelpLine::Heading(text) => Line::styled(
                text,
                Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            HelpLine::Key { keys, text } => Line::from(vec![
                Span::styled(
                    format!("{keys:<16}"),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(text),
            ]),
        })
        .collect();
    frame.render_widget(Paragraph::new(shown), inner);
    let footer = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            "Up/Down PgUp/PgDn scroll · Esc or F1 close",
            Style::new().add_modifier(Modifier::DIM),
        )),
        footer,
    );
}
