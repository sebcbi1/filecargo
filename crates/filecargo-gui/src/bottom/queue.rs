//! The queue, completed and failed lists: one `TableDelegate` for the three, with a progress
//! bar cell for running transfers.

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::{
    App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Stateful, Styled as _,
    Window, div, px, relative,
};

use crate::format::{format_size, format_time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Queue,
    Completed,
    Failed,
}

impl ListKind {
    fn columns(self) -> &'static [(&'static str, f32)] {
        match self {
            Self::Queue => &[
                ("", 28.0),
                ("Name", 240.0),
                ("Progress", 160.0),
                ("Done", 130.0),
                ("Speed", 80.0),
                ("ETA", 60.0),
            ],
            Self::Completed => &[
                ("", 28.0),
                ("Name", 240.0),
                ("Result", 200.0),
                ("Size", 80.0),
                ("Finished", 140.0),
            ],
            Self::Failed => &[("", 28.0), ("Name", 240.0), ("Reason", 380.0), ("", 70.0)],
        }
    }
}

pub struct QueueDelegate {
    pub kind: ListKind,
    handle: AppHandle,
    pub queue: Arc<QueueSnapshot>,
}

impl QueueDelegate {
    pub fn new(kind: ListKind, handle: AppHandle) -> Self {
        Self {
            kind,
            handle,
            queue: Arc::default(),
        }
    }

    pub fn items(&self) -> &[QueueItemView] {
        match self.kind {
            ListKind::Queue => &self.queue.pending,
            ListKind::Completed => &self.queue.completed,
            ListKind::Failed => &self.queue.failed,
        }
    }

    /// The id of the item on `row`.
    pub fn id_at(&self, row: usize) -> Option<TransferId> {
        self.items().get(row).map(|view| view.item.id)
    }
}

/// What a transfer is called in the lists.
pub fn display_name(item: &QueueItem) -> String {
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

fn outcome_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Transferred => "done".to_owned(),
        Outcome::Skipped => "skipped".to_owned(),
        Outcome::Resumed => "resumed".to_owned(),
        Outcome::Renamed(name) => format!("kept both as {name}"),
        Outcome::Created => "folder created".to_owned(),
    }
}

/// `0:12`, `1:02:03`.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// The fraction done (0..=1) of an item with a known size.
pub fn fraction(item: &QueueItem) -> Option<f32> {
    match item.size {
        Some(total) if total > 0 => {
            Some((item.transferred.min(total) as f64 / total as f64) as f32)
        }
        _ => None,
    }
}

impl TableDelegate for QueueDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.kind.columns().len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.items().len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (name, width) = self.kind.columns()[col_ix];
        Column::new(format!("{:?}-{col_ix}", self.kind), name).width(px(width))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(view) = self.items().get(row_ix) else {
            return div().into_any_element();
        };
        let item = &view.item;
        let muted = cx.theme().muted_foreground;
        if col_ix == 0 {
            let icon = match (self.kind, item.direction) {
                (ListKind::Failed, _) => IconName::CircleX,
                (_, Direction::Upload) => IconName::ArrowUp,
                (_, Direction::Download) => IconName::ArrowDown,
            };
            return Icon::new(icon).small().text_color(muted).into_any_element();
        }
        if col_ix == 1 {
            return div().child(display_name(item)).into_any_element();
        }
        match (self.kind, col_ix) {
            (ListKind::Queue, 2) => match &item.state {
                ItemState::Active { .. } => match fraction(item) {
                    Some(done) => h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .w(px(100.))
                                .h(px(8.))
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(done))
                                        .rounded_full()
                                        .bg(cx.theme().primary),
                                ),
                        )
                        .child(format!("{:.0}%", done * 100.0))
                        .into_any_element(),
                    None => div().child("transferring").into_any_element(),
                },
                ItemState::AwaitingDecision { .. } => div()
                    .text_color(cx.theme().warning)
                    .child("waiting for you")
                    .into_any_element(),
                _ => div().text_color(muted).child("queued").into_any_element(),
            },
            (ListKind::Queue, 3) => match (&item.state, item.size) {
                (ItemState::Active { .. }, Some(total)) => div()
                    .child(format!(
                        "{}/{}",
                        format_size(item.transferred),
                        format_size(total)
                    ))
                    .into_any_element(),
                (ItemState::Active { .. }, None) => div()
                    .child(format_size(item.transferred))
                    .into_any_element(),
                (_, Some(total)) => div()
                    .text_color(muted)
                    .child(format_size(total))
                    .into_any_element(),
                _ => div().into_any_element(),
            },
            (ListKind::Queue, 4) => div()
                .child(
                    view.rate
                        .map(|r| format!("{}/s", format_size(r as u64)))
                        .unwrap_or_default(),
                )
                .into_any_element(),
            (ListKind::Queue, 5) => div()
                .child(view.eta.map(format_duration).unwrap_or_default())
                .into_any_element(),
            (ListKind::Completed, 2) => match &item.state {
                ItemState::Completed { outcome, .. } => {
                    div().child(outcome_text(outcome)).into_any_element()
                }
                _ => div().into_any_element(),
            },
            (ListKind::Completed, 3) => div()
                .child(item.size.map(format_size).unwrap_or_default())
                .into_any_element(),
            (ListKind::Completed, 4) => match &item.state {
                ItemState::Completed { finished, .. } => {
                    div().child(format_time(Some(*finished))).into_any_element()
                }
                _ => div().into_any_element(),
            },
            (ListKind::Failed, 2) => match &item.state {
                ItemState::Failed { reason, .. } => div()
                    .text_color(cx.theme().danger)
                    .child(reason.clone())
                    .into_any_element(),
                _ => div().into_any_element(),
            },
            (ListKind::Failed, 3) => match &item.state {
                ItemState::Failed { retryable, .. } => div()
                    .text_color(muted)
                    .child(if *retryable { "retry" } else { "final" })
                    .into_any_element(),
                _ => div().into_any_element(),
            },
            _ => div().into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<gpui_kit::Div> {
        div().id(("row", row_ix))
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(id) = self.id_at(row_ix) else {
            return menu;
        };
        let send = |handle: &AppHandle, command: fn(TransferId) -> Command| {
            let handle = handle.clone();
            move |_: &_, _: &mut Window, _: &mut App| handle.send(command(id))
        };
        let mut menu = menu;
        if self.kind == ListKind::Failed {
            menu = menu.item(
                PopupMenuItem::new("Retry").on_click(send(&self.handle, Command::QueueRetry)),
            );
            let all = self.handle.clone();
            menu = menu.item(
                PopupMenuItem::new("Retry all failed")
                    .on_click(move |_, _, _| all.send(Command::QueueRetryFailed)),
            );
        }
        menu = menu
            .item(PopupMenuItem::new("Remove").on_click(send(&self.handle, Command::QueueRemove)));
        if self.kind == ListKind::Completed {
            let clear = self.handle.clone();
            menu = menu.item(
                PopupMenuItem::new("Clear completed")
                    .on_click(move |_, _, _| clear.send(Command::QueueClearCompleted)),
            );
        }
        menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_fractions() {
        assert_eq!(format_duration(Duration::from_secs(12)), "0:12");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }
}
