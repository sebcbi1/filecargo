//! Drawing of the modal dialogs: a centered box over the screen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::dialog::Dialog;
use crate::form::{Field, FieldKind, Form};
use crate::view::Look;

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn field_line(field: &Field, focused: bool, label_width: usize) -> Line<'static> {
    let label = format!("{:>label_width$} ", format!("{}:", field.label));
    let shown = match &field.kind {
        FieldKind::Text => field.text.clone(),
        FieldKind::Masked => "•".repeat(field.text.chars().count()),
        FieldKind::Checkbox => (if field.checked { "[x]" } else { "[ ]" }).to_owned(),
        FieldKind::Select(_) => format!("‹ {} ›", field.chosen()),
    };
    let mut spans = vec![Span::styled(
        label,
        Style::new().add_modifier(Modifier::BOLD),
    )];
    let text_like = matches!(field.kind, FieldKind::Text | FieldKind::Masked);
    if focused && text_like {
        // the cursor is the reversed character under it (a space past the end)
        let chars: Vec<char> = shown.chars().collect();
        let before: String = chars.iter().take(field.cursor).collect();
        let under = chars.get(field.cursor).copied().unwrap_or(' ');
        let after: String = chars.iter().skip(field.cursor + 1).collect();
        spans.push(Span::raw(before));
        spans.push(Span::styled(
            under.to_string(),
            Style::new().add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::raw(after));
    } else if focused {
        spans.push(Span::styled(
            shown,
            Style::new().add_modifier(Modifier::REVERSED),
        ));
    } else {
        spans.push(Span::raw(shown));
    }
    Line::from(spans)
}

pub(crate) fn render_form(frame: &mut Frame, area: Rect, form: &Form, look: &Look) {
    let visible: Vec<(usize, &Field)> = form
        .fields
        .iter()
        .enumerate()
        .filter(|(_, f)| f.visible)
        .collect();
    let label_width = visible
        .iter()
        .map(|(_, f)| f.label.chars().count() + 1)
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = visible
        .iter()
        .map(|(i, f)| field_line(f, *i == form.focus, label_width))
        .collect();
    lines.push(Line::raw(""));
    match &form.error {
        Some(error) => lines.push(Line::styled(error.clone(), look.tint(Color::Red))),
        None => lines.push(Line::styled(
            "Tab next · Enter OK · Esc cancel",
            Style::new().add_modifier(Modifier::DIM),
        )),
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(crate) fn render(frame: &mut Frame, area: Rect, dialog: &Dialog, look: &Look) {
    let (title, width, height) = match dialog {
        Dialog::Site(editor) => {
            let rows = editor.form.fields.iter().filter(|f| f.visible).count();
            let title = if editor.original.is_some() {
                " Edit site "
            } else {
                " New site "
            };
            (title.to_owned(), 64, rows as u16 + 5)
        }
        Dialog::Input(input) => (format!(" {} ", input.title), 56, 6),
        Dialog::Confirm(confirm) => (format!(" {} ", confirm.title), 56, 6),
        Dialog::Move(picker) => (" Move to ".to_owned(), 48, picker.choices.len() as u16 + 4),
        Dialog::Chmod(chmod) => (
            format!(" Permissions of {} ", names_label(&chmod.names)),
            48,
            chmod.form.fields.len() as u16 + 4,
        ),
        Dialog::Import(_) => (" Import from FileZilla ".to_owned(), 72, 7),
    };
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(look.border(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    match dialog {
        Dialog::Site(editor) => render_form(frame, inner, &editor.form, look),
        Dialog::Input(input) => render_form(frame, inner, &input.form, look),
        Dialog::Chmod(chmod) => render_form(frame, inner, &chmod.form, look),
        Dialog::Import(import) => render_form(frame, inner, &import.form, look),
        Dialog::Confirm(confirm) => {
            let lines = vec![
                Line::raw(confirm.body.clone()),
                Line::raw(""),
                Line::styled(
                    "y / Enter confirm · n / Esc cancel",
                    Style::new().add_modifier(Modifier::DIM),
                ),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Dialog::Move(picker) => {
            let mut lines: Vec<Line> = picker
                .choices
                .iter()
                .enumerate()
                .map(|(i, (_, name))| {
                    let style = if i == picker.cursor {
                        Style::new().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::new()
                    };
                    Line::styled(format!(" {name}"), style)
                })
                .collect();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Up/Down choose · Enter move · Esc cancel",
                Style::new().add_modifier(Modifier::DIM),
            ));
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
}

fn names_label(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        many => format!("{} items", many.len()),
    }
}
