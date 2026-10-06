//! Drawing of the app's prompts as a centered box over the screen.

use filecargo_app_core::prelude::*;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::dialog_view::{centered, render_form};
use crate::pane::{format_size, format_time};
use crate::prompt_ui::PromptUi;
use crate::ui_state::UiState;
use crate::view::Look;

const WIDTH: u16 = 72;

fn dim(text: &str) -> Line<'static> {
    Line::styled(text.to_owned(), Style::new().add_modifier(Modifier::DIM))
}

fn bold(text: String) -> Line<'static> {
    Line::styled(text, Style::new().add_modifier(Modifier::BOLD))
}

fn side(entry: &Entry, ui: &UiState) -> String {
    format!(
        "{:>8}  {}",
        format_size(entry.size),
        format_time(entry.modified, ui.now)
    )
}

/// Title and body lines of a prompt that is not a form.
fn text_of(
    kind: &PromptKind,
    pui: &PromptUi,
    ui: &UiState,
    look: &Look,
) -> (String, Vec<Line<'static>>) {
    match kind {
        PromptKind::HostKey(p) => (
            " Unknown host key ".to_owned(),
            vec![
                Line::raw(format!(
                    "The authenticity of {}:{} cannot be established.",
                    p.host, p.port
                )),
                Line::raw(format!("{} key fingerprint:", p.algorithm)),
                bold(p.fingerprint.clone()),
                Line::raw(""),
                dim("y trust once · a trust always · n / Esc reject"),
            ],
        ),
        PromptKind::Certificate(p) => (
            " Untrusted certificate ".to_owned(),
            vec![
                Line::styled(
                    format!("{}:{} — {}.", p.host, p.port, p.problem),
                    look.tint(Color::Yellow),
                ),
                Line::raw(format!("Subject: {}", p.subject)),
                Line::raw(format!("Issuer:  {}", p.issuer)),
                Line::raw(format!("Expires: {}", p.not_after)),
                Line::raw("SHA-256:"),
                bold(p.sha256.clone()),
                Line::raw(""),
                dim("y trust once · a trust always · n / Esc reject"),
            ],
        ),
        PromptKind::Conflict { conflict, .. } => {
            let apply_all = matches!(pui, PromptUi::Conflict { apply_all: true });
            (
                " File exists ".to_owned(),
                vec![
                    bold(conflict.source.name.clone()),
                    Line::raw(format!("New:      {}", side(&conflict.source, ui))),
                    Line::raw(format!("Existing: {}", side(&conflict.target, ui))),
                    Line::raw(""),
                    Line::raw(format!(
                        "[{}] a  apply to all remaining files",
                        if apply_all { "x" } else { " " }
                    )),
                    dim("o overwrite · n if newer · r resume · s skip · k keep both · Esc skip"),
                ],
            )
        }
        PromptKind::ConfirmDelete {
            names, recursive, ..
        } => {
            let what = match names.as_slice() {
                [one] => format!("\"{one}\""),
                many => format!("{} items", many.len()),
            };
            let mut lines = vec![Line::raw(format!("Delete {what}?"))];
            if *recursive {
                lines.push(Line::styled(
                    "Folders are deleted with everything in them.",
                    look.tint(Color::Yellow),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(dim("y / Enter delete · n / Esc cancel"));
            (" Delete ".to_owned(), lines)
        }
        PromptKind::ConfirmQuit { active_transfers } => (
            " Quit ".to_owned(),
            vec![
                Line::raw(format!(
                    "{active_transfers} transfer{} still running. Quit anyway?",
                    if *active_transfers == 1 {
                        " is"
                    } else {
                        "s are"
                    }
                )),
                Line::raw(""),
                dim("y / Enter quit · n / Esc stay"),
            ],
        ),
        PromptKind::ConfirmClearQueue { items, active } => {
            let mut lines = vec![Line::raw(format!(
                "Clear {items} item{} from this site's queue?",
                if *items == 1 { "" } else { "s" }
            ))];
            if *active > 0 {
                lines.push(Line::styled(
                    format!(
                        "{active} running transfer{} will be cancelled; partial files are kept.",
                        if *active == 1 { "" } else { "s" }
                    ),
                    look.tint(Color::Yellow),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(dim("y / Enter clear · n / Esc cancel"));
            (" Clear queue ".to_owned(), lines)
        }
        PromptKind::Message { level, title, body } => {
            let color = match level {
                Level::Info => Color::Reset,
                Level::Warning => Color::Yellow,
                Level::Error => Color::Red,
            };
            let mut lines: Vec<Line> = body
                .lines()
                .map(|l| Line::styled(l.to_owned(), look.tint(color)))
                .collect();
            lines.push(Line::raw(""));
            lines.push(dim("Enter to close"));
            (format!(" {title} "), lines)
        }
        PromptKind::Credential(_) => (String::new(), Vec::new()),
    }
}

fn credential_header(prompt: &CredentialPrompt) -> (String, Vec<Line<'static>>) {
    match prompt {
        CredentialPrompt::Password { site, user, retry } => (
            format!(" Password for {user}@{site} "),
            retry
                .then(|| bold("Login failed, try again.".to_owned()))
                .into_iter()
                .collect(),
        ),
        CredentialPrompt::Passphrase {
            site,
            key_path,
            retry,
        } => (
            format!(" Key passphrase ({site}) "),
            [
                Some(Line::raw(key_path.display().to_string())),
                retry.then(|| bold("Wrong passphrase, try again.".to_owned())),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        CredentialPrompt::KeyboardInteractive {
            site,
            name,
            instructions,
            ..
        } => (
            format!(" {} ", if name.is_empty() { site } else { name }),
            instructions
                .lines()
                .map(|l| Line::raw(l.to_owned()))
                .collect(),
        ),
    }
}

pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    kind: &PromptKind,
    pui: &PromptUi,
    ui: &UiState,
    look: &Look,
) {
    if let (PromptKind::Credential(prompt), PromptUi::Credential(form)) = (kind, pui) {
        let (title, header) = credential_header(prompt);
        let fields = form.fields.iter().filter(|f| f.visible).count() as u16;
        let rect = centered(area, WIDTH, header.len() as u16 + fields + 4);
        frame.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(look.border(true));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let top = Rect {
            height: header.len() as u16,
            ..inner
        };
        frame.render_widget(Paragraph::new(header), top);
        let rest = Rect {
            y: inner.y + top.height,
            height: inner.height.saturating_sub(top.height),
            ..inner
        };
        render_form(frame, rest, form, look);
        return;
    }
    let (title, lines) = text_of(kind, pui, ui, look);
    let inner_width = usize::from(WIDTH) - 2;
    let rows: usize = lines
        .iter()
        .map(|l| (l.width().max(1)).div_ceil(inner_width))
        .sum();
    let rect = centered(area, WIDTH, rows as u16 + 2);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(look.border(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
