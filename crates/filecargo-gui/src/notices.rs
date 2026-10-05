//! `AppState.notices` as gpui-kit notifications: information fades by itself, errors stay until
//! closed; closing one tells the app.

use std::collections::HashSet;

use filecargo_app_core::prelude::*;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window, div};

use crate::model::AppModel;

pub struct NoticeHost {
    model: Entity<AppModel>,
    seen: HashSet<NoticeId>,
    pending_before: usize,
    done_before_batch: usize,
    _subscription: Subscription,
}

impl NoticeHost {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _subscription =
            cx.observe_in(&model, window, |this, _, window, cx| this.sync(window, cx));
        let mut host = Self {
            model,
            seen: HashSet::new(),
            pending_before: 0,
            done_before_batch: 0,
            _subscription,
        };
        host.sync(window, cx);
        host
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notices = self.model.read(cx).state.notices.clone();
        self.batch(window, cx);
        // forget the ones the app no longer lists
        self.seen.retain(|id| notices.iter().any(|n| n.id == *id));
        for notice in notices {
            if !self.seen.insert(notice.id) {
                continue;
            }
            let id = notice.id;
            let model = self.model.clone();
            let notification = match notice.level {
                Level::Info => Notification::info(notice.text),
                Level::Warning => Notification::warning(notice.text),
                // errors stay until the user closes them
                Level::Error => Notification::error(notice.text).autohide(false),
            }
            .on_close(move |_, cx| {
                model.read(cx).send(Command::DismissNotice(id));
            });
            window.push_notification(notification, cx);
        }
    }
}

impl Render for NoticeHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What the "transfers finished" notification says; `None` when nothing finished in the batch.
pub fn batch_summary(done: usize, failed: usize) -> Option<(String, String)> {
    if done == 0 && failed == 0 {
        return None;
    }
    let title = if failed == 0 {
        "Transfers finished"
    } else {
        "Transfers finished with errors"
    };
    let mut message = format!(
        "{done} item{} transferred",
        if done == 1 { "" } else { "s" }
    );
    if failed > 0 {
        message.push_str(&format!(", {failed} failed"));
    }
    Some((title.to_owned(), message))
}

/// Whether a batch just ended while the user is elsewhere: the queue went from busy to empty
/// and the window is not the active one.
pub fn batch_ended(pending_before: usize, pending_now: usize, window_active: bool) -> bool {
    pending_before > 0 && pending_now == 0 && !window_active
}

impl NoticeHost {
    /// Posts an OS notification when a whole batch of transfers ends while the window is in the
    /// background.
    fn batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let queue = self.model.read(cx).state.queue.clone();
        let pending = queue.pending.len();
        if self.pending_before == 0 && pending > 0 {
            self.done_before_batch = queue.completed.len();
        }
        if batch_ended(self.pending_before, pending, window.is_window_active()) {
            let done = queue.completed.len().saturating_sub(self.done_before_batch);
            if let Some((title, message)) = batch_summary(done, queue.failed.len()) {
                window.push_notification(
                    Notification::new().title(title).message(message).system(),
                    cx,
                );
            }
        }
        self.pending_before = pending;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_and_flags_failures() {
        assert_eq!(batch_summary(0, 0), None);
        assert_eq!(
            batch_summary(1, 0),
            Some(("Transfers finished".into(), "1 item transferred".into()))
        );
        assert_eq!(
            batch_summary(3, 2),
            Some((
                "Transfers finished with errors".into(),
                "3 items transferred, 2 failed".into()
            ))
        );
    }

    #[test]
    fn only_an_unfocused_window_is_told_about_the_end_of_a_batch() {
        assert!(batch_ended(2, 0, false));
        assert!(!batch_ended(2, 0, true), "the user is looking at it");
        assert!(!batch_ended(2, 1, false), "still busy");
        assert!(!batch_ended(0, 0, false), "nothing was running");
    }
}
