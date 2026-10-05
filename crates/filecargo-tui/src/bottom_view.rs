//! The bottom panel: tabs with counts, the transfer queue with progress bars, the completed and
//! failed lists, and the log.

use filecargo_app_core::prelude::*;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::pane::{format_clock, format_duration, format_rate, format_size, format_time};
use crate::ui_state::{BottomTab, Focus, ListUi, UiState};
use crate::view::Look;

const BAR: usize = 10;

fn tab_title(ui: &UiState, app: &AppState, look: &Look) -> Line<'static> {
    let focused = ui.focus == Focus::Bottom;
    let mut spans = Vec::new();
    for tab in BottomTab::ALL {
        let count = match tab {
            BottomTab::Queue => Some(app.queue.pending.len()),
            BottomTab::Completed => Some(app.queue.completed.len()),
            BottomTab::Failed => Some(app.queue.failed.len()),
            BottomTab::Log | BottomTab::Terminal => None,
        };
        let text = match count {
            Some(n) => format!(" {} ({n}) ", tab.name()),
            None => format!(" {} ", tab.name()),
        };
        let mut style = Style::new();
        if tab == BottomTab::Failed && count.unwrap_or(0) > 0 {
            style = style.patch(look.tint(Color::Red));
        }
        if tab == ui.bottom.tab {
            style = style.add_modifier(if focused {
                Modifier::REVERSED | Modifier::BOLD
            } else {
                Modifier::BOLD | Modifier::UNDERLINED
            });
        }
        spans.push(Span::styled(text, style));
        spans.push(Span::raw("│"));
    }
    spans.pop();
    Line::from(spans)
}

pub(crate) fn render(frame: &mut Frame, area: Rect, ui: &UiState, app: &AppState, look: &Look) {
    let focused = ui.focus == Focus::Bottom;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(tab_title(ui, app, look))
        .border_style(look.border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match ui.bottom.tab {
        BottomTab::Queue => render_queue(frame, inner, ui, app, look),
        BottomTab::Completed => render_completed(frame, inner, ui, app, look),
        BottomTab::Failed => render_failed(frame, inner, ui, app, look),
        BottomTab::Log => render_log(frame, inner, ui, look),
        BottomTab::Terminal => render_terminal(frame, inner, app, look),
    }
}

fn name_of(item: &QueueItem) -> String {
    let name = match item.direction {
        Direction::Upload => item
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        Direction::Download => item.remote.file_name().map(str::to_owned),
    }
    .unwrap_or_else(|| item.remote.to_string());
    if item.is_dir {
        format!("{name}/")
    } else {
        name
    }
}

fn arrow(direction: Direction) -> &'static str {
    match direction {
        Direction::Upload => "↑",
        Direction::Download => "↓",
    }
}

fn bar(done: u64, total: u64) -> String {
    let done = done.min(total);
    let pct = (done * 100).checked_div(total).unwrap_or(0);
    let filled = usize::try_from(pct).unwrap_or(0) * BAR / 100;
    format!(
        "{}{} {pct:>3}%",
        "█".repeat(filled),
        "░".repeat(BAR - filled)
    )
}

fn row_style(selected: bool, focused: bool) -> Style {
    if selected {
        Style::new().add_modifier(if focused {
            Modifier::REVERSED
        } else {
            Modifier::UNDERLINED
        })
    } else {
        Style::new()
    }
}

fn visible<T>(items: &[T], list: ListUi, rows: usize) -> impl Iterator<Item = (usize, &T)> {
    items.iter().enumerate().skip(list.offset).take(rows)
}

fn empty(frame: &mut Frame, area: Rect, text: &str) {
    let middle = Rect {
        y: area.y + area.height / 2,
        height: 1.min(area.height),
        ..area
    };
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), middle);
}

fn render_queue(frame: &mut Frame, area: Rect, ui: &UiState, app: &AppState, look: &Look) {
    let queue = &app.queue;
    if queue.pending.is_empty() {
        empty(
            frame,
            area,
            "Nothing queued. Select files and press F5 to transfer.",
        );
        return;
    }
    let focused = ui.focus == Focus::Bottom;
    let rows_visible = usize::from(area.height.saturating_sub(2));
    let header = Row::new(["", "Name", "Progress", "Done", "Speed", "ETA"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let mut rows = Vec::new();
    for (index, view) in visible(&queue.pending, ui.bottom.queue, rows_visible) {
        let item = &view.item;
        let (progress, done, speed, eta) = match &item.state {
            ItemState::Active { .. } => match item.size {
                Some(total) if total > 0 => (
                    bar(item.transferred, total),
                    format!("{}/{}", format_size(item.transferred), format_size(total)),
                    view.rate.map(format_rate).unwrap_or_default(),
                    view.eta.map(format_duration).unwrap_or_default(),
                ),
                _ => (
                    "transferring".to_owned(),
                    format_size(item.transferred),
                    view.rate.map(format_rate).unwrap_or_default(),
                    String::new(),
                ),
            },
            ItemState::AwaitingDecision { .. } => (
                "waiting for you".to_owned(),
                String::new(),
                String::new(),
                String::new(),
            ),
            _ => (
                "queued".to_owned(),
                item.size.map(format_size).unwrap_or_default(),
                String::new(),
                String::new(),
            ),
        };
        let mut style = row_style(index == ui.bottom.queue.cursor, focused);
        if matches!(item.state, ItemState::Active { .. }) {
            style = style.patch(look.tint(Color::Green));
        }
        rows.push(
            Row::new([
                Cell::from(arrow(item.direction)),
                Cell::from(name_of(item)),
                Cell::from(progress),
                Cell::from(Line::from(done).alignment(Alignment::Right)),
                Cell::from(Line::from(speed).alignment(Alignment::Right)),
                Cell::from(Line::from(eta).alignment(Alignment::Right)),
            ])
            .style(style),
        );
    }
    let table_area = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length((BAR + 5) as u16),
                Constraint::Length(17),
                Constraint::Length(9),
                Constraint::Length(7),
            ],
        )
        .header(header),
        table_area,
    );
    let totals = &queue.totals;
    let mut footer = format!(
        "Total {}/{}",
        format_size(totals.bytes_done),
        format_size(totals.bytes_total)
    );
    if let Some(rate) = totals.rate {
        footer.push_str(&format!(" · {}", format_rate(rate)));
    }
    if let Some(eta) = totals.eta {
        footer.push_str(&format!(" · ETA {}", format_duration(eta)));
    }
    let footer_style = if queue.processing {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        footer.push_str(" · PAUSED (p to resume)");
        look.tint(Color::Yellow)
    };
    let line = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    frame.render_widget(Paragraph::new(Span::styled(footer, footer_style)), line);
}

fn outcome_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Transferred => "done".to_owned(),
        Outcome::Skipped => "skipped".to_owned(),
        Outcome::Resumed => "resumed".to_owned(),
        Outcome::Renamed(name) => format!("kept both as {name}"),
        Outcome::Created => "folder created".to_owned(),
    }
}

fn render_completed(frame: &mut Frame, area: Rect, ui: &UiState, app: &AppState, look: &Look) {
    let list = &app.queue.completed;
    if list.is_empty() {
        empty(frame, area, "Nothing completed yet.");
        return;
    }
    let focused = ui.focus == Focus::Bottom;
    let rows_visible = usize::from(area.height.saturating_sub(2));
    let mut rows = Vec::new();
    for (index, view) in visible(list, ui.bottom.completed, rows_visible) {
        let item = &view.item;
        let (result, when) = match &item.state {
            ItemState::Completed { outcome, finished } => {
                (outcome_text(outcome), format_time(Some(*finished), ui.now))
            }
            _ => (String::new(), String::new()),
        };
        rows.push(
            Row::new([
                Cell::from(arrow(item.direction)),
                Cell::from(name_of(item)),
                Cell::from(result),
                Cell::from(
                    Line::from(item.size.map(format_size).unwrap_or_default())
                        .alignment(Alignment::Right),
                ),
                Cell::from(when),
            ])
            .style(
                row_style(index == ui.bottom.completed.cursor, focused)
                    .patch(look.tint(Color::Reset)),
            ),
        );
    }
    let header = Row::new(["", "Name", "Result", "Size", "Finished"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(24),
                Constraint::Length(8),
                Constraint::Length(11),
            ],
        )
        .header(header),
        area,
    );
    let line = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            "c clear the list",
            Style::new().add_modifier(Modifier::DIM),
        )),
        line,
    );
}

fn render_failed(frame: &mut Frame, area: Rect, ui: &UiState, app: &AppState, look: &Look) {
    let list = &app.queue.failed;
    if list.is_empty() {
        empty(frame, area, "No failed transfers.");
        return;
    }
    let focused = ui.focus == Focus::Bottom;
    let rows_visible = usize::from(area.height.saturating_sub(2));
    let mut rows = Vec::new();
    for (index, view) in visible(list, ui.bottom.failed, rows_visible) {
        let item = &view.item;
        let (reason, retryable) = match &item.state {
            ItemState::Failed {
                reason, retryable, ..
            } => (reason.clone(), *retryable),
            _ => (String::new(), false),
        };
        rows.push(
            Row::new([
                Cell::from("✗"),
                Cell::from(name_of(item)),
                Cell::from(reason),
                Cell::from(if retryable { "retry" } else { "final" }),
            ])
            .style(
                row_style(index == ui.bottom.failed.cursor, focused).patch(look.tint(Color::Red)),
            ),
        );
    }
    let header =
        Row::new(["", "Name", "Reason", ""]).style(Style::new().add_modifier(Modifier::BOLD));
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Length(24),
                Constraint::Min(10),
                Constraint::Length(6),
            ],
        )
        .header(header),
        area,
    );
    let line = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            "r retry · R retry all · Del remove",
            Style::new().add_modifier(Modifier::DIM),
        )),
        line,
    );
}

fn render_log(frame: &mut Frame, area: Rect, ui: &UiState, look: &Look) {
    let rows = usize::from(area.height);
    let lines: Vec<Line> = ui.log.with_lines(|lines| {
        let end = lines.len().saturating_sub(ui.bottom.log_scroll);
        let start = end.saturating_sub(rows);
        lines
            .iter()
            .skip(start)
            .take(end - start)
            .map(|line| {
                let (label, color) = match line.level {
                    LogLevel::Error => ("ERROR", Color::Red),
                    LogLevel::Warn => ("WARN ", Color::Yellow),
                    LogLevel::Info => ("INFO ", Color::Reset),
                    LogLevel::Debug => ("DEBUG", Color::Blue),
                    LogLevel::Trace => ("TRACE", Color::DarkGray),
                };
                Line::from(vec![
                    Span::styled(
                        format_clock(line.time),
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                    Span::raw(" "),
                    Span::styled(label, look.tint(color).add_modifier(Modifier::BOLD)),
                    Span::raw(format!(" {} {}", line.target, line.message)),
                ])
            })
            .collect()
    });
    if lines.is_empty() {
        empty(frame, area, "The log is empty.");
    } else {
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn draw_screen(frame: &mut Frame, area: Rect, view: &TerminalView) {
    view.handle.with_screen(|screen| {
        frame.render_widget(tui_term::widget::PseudoTerminal::new(screen), area);
        let back = screen.scrollback();
        if back > 0 {
            let label = format!(" ↑ {back} lines back · type to return ");
            let width = label.chars().count() as u16;
            if area.width > width {
                let at = Rect {
                    x: area.x + area.width - width,
                    y: area.y,
                    width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        label,
                        Style::new().add_modifier(Modifier::REVERSED),
                    )),
                    at,
                );
            }
        }
    });
}

fn render_terminal(frame: &mut Frame, area: Rect, app: &AppState, look: &Look) {
    match &app.terminal {
        TerminalState::Open(view) => draw_screen(frame, area, view),
        TerminalState::Exited { code, view } => {
            draw_screen(
                frame,
                Rect {
                    height: area.height.saturating_sub(1),
                    ..area
                },
                view,
            );
            let status = match code {
                Some(code) => {
                    format!("Shell exited (status {code}). Enter to reopen · Ctrl-\\ to leave")
                }
                None => "Shell ended. Enter to reopen · Ctrl-\\ to leave".to_owned(),
            };
            let line = Rect {
                y: area.y + area.height - 1,
                height: 1,
                ..area
            };
            frame.render_widget(
                Paragraph::new(Span::styled(status, look.tint(Color::Yellow))),
                line,
            );
        }
        TerminalState::Closed => empty(frame, area, "Opening the shell…"),
        TerminalState::NotAvailable => {
            empty(
                frame,
                area,
                "The terminal needs an SFTP connection (FTP has no shell).",
            );
        }
    }
}
