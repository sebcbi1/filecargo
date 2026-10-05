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
    _subscription: Subscription,
}

impl NoticeHost {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _subscription =
            cx.observe_in(&model, window, |this, _, window, cx| this.sync(window, cx));
        let mut host = Self {
            model,
            seen: HashSet::new(),
            _subscription,
        };
        host.sync(window, cx);
        host
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notices = self.model.read(cx).state.notices.clone();
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
