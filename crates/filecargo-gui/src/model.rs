//! `AppModel`: the latest `AppState` snapshot as a gpui entity.

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use gpui_kit::{AppContext as _, Context, Entity};

/// How often the model looks for a new snapshot. app-core publishes at most 30 a second, so
/// this adds no visible latency. (Awaiting `watch::changed()` would wake the gpui task from
/// the app-core thread, which gpui's deterministic test scheduler rejects, so tests could not
/// cover the same code that runs in production.)
pub const POLL: Duration = Duration::from_millis(33);

pub struct AppModel {
    pub handle: AppHandle,
    pub state: Arc<AppState>,
}

impl AppModel {
    /// Starts a foreground task that stores every new snapshot and notifies the views. The task
    /// ends, quitting the application, when the snapshot channel closes (the app has quit), or
    /// when the entity is dropped.
    pub fn new(handle: AppHandle, cx: &mut gpui_kit::App) -> Entity<Self> {
        let mut snapshots = handle.state();
        let state = snapshots.borrow_and_update().clone();
        cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    match snapshots.has_changed() {
                        Err(_) => {
                            this.update(cx, |_, cx| cx.quit()).ok();
                            break;
                        }
                        Ok(false) => {}
                        Ok(true) => {
                            let next = snapshots.borrow_and_update().clone();
                            let gone = this
                                .update(cx, |model, cx| {
                                    model.state = next;
                                    cx.notify();
                                })
                                .is_err();
                            if gone {
                                break;
                            }
                        }
                    }
                }
            })
            .detach();
            Self { handle, state }
        })
    }

    pub fn send(&self, command: Command) {
        self.handle.send(command);
    }
}
