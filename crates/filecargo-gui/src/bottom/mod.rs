//! The bottom panel: tabs for the queue, completed and failed transfers, the log and (T7) the
//! terminal.

pub mod log;
pub mod queue;

use std::sync::Arc;

use filecargo_app_core::prelude::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};

use crate::model::AppModel;
use crate::terminal::TerminalView;
use log::LogView;
use queue::{ListKind, QueueDelegate};

gpui_kit::actions!(filecargo_bottom, [RemoveItem]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab_ {
    Queue = 0,
    Completed = 1,
    Failed = 2,
    Log = 3,
    Terminal = 4,
}

impl Tab_ {
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Queue,
            1 => Self::Completed,
            2 => Self::Failed,
            3 => Self::Log,
            _ => Self::Terminal,
        }
    }
}

pub struct BottomPanel {
    model: Entity<AppModel>,
    pub tab: usize,
    pub queue: Entity<TableState<QueueDelegate>>,
    pub completed: Entity<TableState<QueueDelegate>>,
    pub failed: Entity<TableState<QueueDelegate>>,
    pub log: Entity<LogView>,
    pub terminal: Entity<TerminalView>,
    shown: Option<Arc<QueueSnapshot>>,
    _subscription: Subscription,
}

impl BottomPanel {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let handle = model.read(cx).handle.clone();
        let table = |kind: ListKind, window: &mut Window, cx: &mut Context<Self>| {
            let handle = handle.clone();
            cx.new(|cx| TableState::new(QueueDelegate::new(kind, handle), window, cx))
        };
        let queue = table(ListKind::Queue, window, cx);
        let completed = table(ListKind::Completed, window, cx);
        let failed = table(ListKind::Failed, window, cx);
        let log_buffer = handle.log();
        let log = cx.new(|_| LogView::new(log_buffer));
        let terminal = cx.new(|cx| TerminalView::new(model.clone(), cx));
        let _subscription = cx.observe(&model, |this, _, cx| this.sync(cx));
        let mut panel = Self {
            model,
            tab: 0,
            queue,
            completed,
            failed,
            log,
            terminal,
            shown: None,
            _subscription,
        };
        panel.sync(cx);
        panel
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let queue = self.model.read(cx).state.site_queue.clone();
        let changed = self
            .shown
            .as_ref()
            .is_none_or(|shown| !Arc::ptr_eq(shown, &queue));
        if changed {
            for table in [&self.queue, &self.completed, &self.failed] {
                let queue = queue.clone();
                table.update(cx, |table, cx| {
                    table.delegate_mut().queue = queue;
                    cx.notify();
                });
            }
            self.shown = Some(queue);
        }
        // the log tab redraws on every snapshot (its generation is part of the state)
        let scope = self.model.read(cx).state.scope;
        self.log.update(cx, |log, cx| {
            log.scope = scope;
            cx.notify();
        });
        cx.notify();
    }

    pub fn select_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.tab = tab.min(4);
        cx.notify();
    }

    /// The Queue tab's buttons: start the held items, pause or resume this site, clear its queue.
    fn queue_bar(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let state = self.model.read(cx).state.clone();
        let connected = state.scope.is_some();
        let held = state
            .site_queue
            .pending
            .iter()
            .any(|v| matches!(v.item.state, ItemState::Held));
        let paused = state.site_paused();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .child(
                Button::new("queue-start")
                    .small()
                    .icon(IconName::Play)
                    .label("Start queue")
                    .disabled(!held && !paused)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.read(cx).send(Command::QueueStartHeld);
                    })),
            )
            .child(
                Button::new("queue-pause")
                    .small()
                    .icon(if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .label(if paused { "Resume" } else { "Pause" })
                    .disabled(!connected)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let paused = this.model.read(cx).state.site_paused();
                        this.model
                            .read(cx)
                            .send(Command::QueueSetSitePaused(!paused));
                    })),
            )
            .child(
                Button::new("queue-clear")
                    .small()
                    .icon(IconName::Delete)
                    .label("Clear queue")
                    .disabled(state.site_queue.pending.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.read(cx).send(Command::QueueClear);
                    })),
            )
    }

    /// Removes the item under the cursor of the list on show.
    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let table = match self.tab {
            0 => &self.queue,
            1 => &self.completed,
            2 => &self.failed,
            _ => return,
        };
        let id = table
            .read(cx)
            .selected_row()
            .and_then(|row| table.read(cx).delegate().id_at(row));
        if let Some(id) = id {
            self.model.read(cx).send(Command::QueueRemove(id));
        }
    }
}

impl Render for BottomPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.model.read(cx).state.clone();
        let queue = state.site_queue.clone();
        let label = |name: &str, count: usize| {
            if count == 0 {
                name.to_owned()
            } else {
                format!("{name} ({count})")
            }
        };
        let tabs = TabBar::new("bottom-tabs")
            .selected_index(self.tab)
            .children([
                Tab::new().label(if state.site_paused() {
                    format!("Queue ({}, paused)", queue.pending.len())
                } else {
                    label("Queue", queue.pending.len())
                }),
                Tab::new().label(label("Completed", queue.completed.len())),
                Tab::new().label(label("Failed", queue.failed.len())),
                Tab::new().label("Log"),
                Tab::new().label("Terminal"),
            ])
            .on_click(cx.listener(|this, index: &usize, window, cx| {
                this.select_tab(*index, cx);
                if *index == 4 {
                    this.terminal
                        .clone()
                        .update(cx, |t, cx| t.focus(window, cx));
                }
            }));
        let body = match self.tab {
            0 => DataTable::new(&self.queue).stripe(true).into_any_element(),
            1 => DataTable::new(&self.completed)
                .stripe(true)
                .into_any_element(),
            2 => DataTable::new(&self.failed).stripe(true).into_any_element(),
            3 => self.log.clone().into_any_element(),
            _ => self.terminal.clone().into_any_element(),
        };
        v_flex()
            .id("bottom-panel")
            .key_context("Queue")
            .size_full()
            .on_action(cx.listener(|this, _: &RemoveItem, _, cx| this.remove_selected(cx)))
            .child(tabs)
            .children((self.tab == 0).then(|| self.queue_bar(cx)))
            .child(div().flex_1().child(body))
    }
}
