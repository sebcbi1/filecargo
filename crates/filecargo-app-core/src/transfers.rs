//! The transfer queue inside the app: connections for its workers, commands that build
//! transfers from pane selections, and its events turned into snapshots and prompts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filecargo_config::{ConnectionSettings, ServerTree, SiteId};
use filecargo_remote_fs::{ConnectContext, EntryKind, RemoteFs};
use filecargo_transfer::{
    ConflictDecision, Connector, Direction, ItemState, NewTransfer, Queue, QueueEvent, QueueLimits,
    TransferError, TransferId,
};
use tokio::time::Instant;

use crate::app::{Core, Msg};
use crate::command::Command;
use crate::logging::site_span;
use crate::prompt::PromptAction;
use crate::session::SessionFactory;
use crate::state::{Level, PromptAnswer, PromptKind};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// What the connector needs to know that changes while the app runs.
pub(crate) struct Shared {
    pub tree: Mutex<Arc<ServerTree>>,
    pub timeouts: Mutex<ConnectionSettings>,
}

/// Opens connections for the queue's workers through the app's `SessionFactory` and the same
/// `ConnectContext` as the browsing session, so nobody is asked twice for the same answer.
pub(crate) struct QueueConnector {
    pub factory: Arc<dyn SessionFactory>,
    pub ctx: ConnectContext,
    pub shared: Arc<Shared>,
}

#[async_trait]
impl Connector for QueueConnector {
    async fn connect(&self, site: SiteId) -> Result<Arc<dyn RemoteFs>, TransferError> {
        let site = self
            .shared
            .tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .site(site)
            .cloned()
            .ok_or(TransferError::SiteDeleted)?;
        let mut ctx = self.ctx.clone();
        ctx.timeouts = self
            .shared
            .timeouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.factory
            .connect(&site, &ctx)
            .await
            .map(|session| session.fs)
            .map_err(|e| TransferError::Connect(e.to_string()))
    }
}

/// When a pane may next be refreshed after transfers finished into it.
#[derive(Default)]
pub(crate) struct RefreshSchedule {
    pub due: Option<Instant>,
    pub last: Option<Instant>,
}

impl RefreshSchedule {
    /// A transfer finished into the pane: refresh soon, but at most once a second.
    fn request(&mut self) {
        if self.due.is_none() {
            let now = Instant::now();
            self.due = Some(
                self.last
                    .map_or(now, |last| (last + REFRESH_INTERVAL).max(now)),
            );
        }
    }

    fn take_if_due(&mut self) -> bool {
        match self.due {
            Some(at) if at <= Instant::now() => {
                self.due = None;
                self.last = Some(Instant::now());
                true
            }
            _ => false,
        }
    }
}

impl Core {
    pub(crate) fn queue_limits(&self) -> QueueLimits {
        QueueLimits {
            max_concurrent: self.state.settings.transfers.max_concurrent,
            default_conflict: self.state.settings.transfers.default_conflict,
        }
    }

    /// Keeps what the queue's workers see (sites, timeouts) in step with the app.
    pub(crate) fn sync_shared(&self) {
        *self.shared.tree.lock().unwrap_or_else(|e| e.into_inner()) = self.state.servers.clone();
        *self
            .shared
            .timeouts
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.state.settings.connection.clone();
    }

    /// Starts the queue and forwards its events to the actor.
    pub(crate) async fn start_queue(&mut self) {
        let connector = Arc::new(QueueConnector {
            factory: self.factory.clone(),
            ctx: self.ctx.clone(),
            shared: self.shared.clone(),
        });
        match Queue::start(connector, self.paths.queue(), self.queue_limits()).await {
            Ok((queue, mut events)) => {
                self.state.queue = queue.snapshot();
                self.queue = Some(queue);
                let messages = self.messages.clone();
                tokio::spawn(async move {
                    while let Some(event) = events.recv().await {
                        if messages.send(Msg::Queue(event)).is_err() {
                            break;
                        }
                    }
                });
                self.changed();
            }
            Err(error) => {
                tracing::error!(target: "filecargo::app", %error, "the transfer queue could not start");
                self.notice(
                    Level::Error,
                    format!("The transfer queue could not start: {error}"),
                );
            }
        }
    }

    pub(crate) fn on_queue_event(&mut self, event: QueueEvent) {
        let Some(queue) = &self.queue else { return };
        match event {
            QueueEvent::Changed => {
                self.state.queue = queue.snapshot();
                self.changed();
            }
            QueueEvent::ConflictAsked { id, conflict } => {
                self.state.queue = queue.snapshot();
                let prompt = self.enqueue_prompt(PromptKind::Conflict {
                    transfer: id,
                    conflict,
                });
                self.actions
                    .insert(prompt, PromptAction::Conflict { transfer: id });
            }
            QueueEvent::ItemFinished { id, ok } => {
                self.state.queue = queue.snapshot();
                self.changed();
                if ok {
                    self.note_finished_into_panes(id);
                } else if let Some(reason) = self.failure_reason(id) {
                    let _span = self.failed_site(id).map(|site| site_span(site).entered());
                    tracing::warn!(target: "filecargo::app", %id, %reason, "a transfer failed");
                }
            }
            QueueEvent::Idle => {
                self.state.queue = queue.snapshot();
                self.changed();
                // the last files of a batch must show up even inside the debounce window
                self.local_refresh.request();
                self.remote_refresh.request();
            }
        }
    }

    fn failed_site(&self, id: TransferId) -> Option<SiteId> {
        let failed = &self.state.queue.failed;
        failed.iter().find(|v| v.item.id == id).map(|v| v.item.site)
    }

    fn failure_reason(&self, id: TransferId) -> Option<String> {
        self.state
            .queue
            .failed
            .iter()
            .find(|v| v.item.id == id)
            .and_then(|v| match &v.item.state {
                ItemState::Failed { reason, .. } => Some(reason.clone()),
                _ => None,
            })
    }

    /// If the finished item landed in the directory a pane shows, that pane refreshes soon.
    fn note_finished_into_panes(&mut self, id: TransferId) {
        let Some(view) = self.state.queue.completed.iter().find(|v| v.item.id == id) else {
            return;
        };
        let item = &view.item;
        match item.direction {
            Direction::Upload => {
                let shown = self.state.remote.as_ref().map(|p| &p.path);
                if shown.is_some() && item.remote.parent().as_ref() == shown {
                    self.remote_refresh.request();
                }
            }
            Direction::Download => {
                if item.local.parent() == Some(self.state.local.path.as_path()) {
                    self.local_refresh.request();
                }
            }
        }
    }

    /// Runs the refreshes that came due; the next due time feeds the actor's wake-up.
    pub(crate) fn run_due_refreshes(&mut self) {
        if self.local_refresh.take_if_due() {
            self.refresh_local();
        }
        if self.remote_refresh.take_if_due() {
            self.refresh_remote();
        }
    }

    pub(crate) fn next_refresh_due(&self) -> Option<Instant> {
        [self.local_refresh.due, self.remote_refresh.due]
            .into_iter()
            .flatten()
            .min()
    }

    // ---- commands -------------------------------------------------------------------------------

    /// `Upload` / `Download`: one transfer per selected name of the source pane.
    pub(crate) fn start_transfers(&mut self, direction: Direction, names: &[String]) {
        self.queue_transfers(direction, names, false);
    }

    /// `Enqueue`: like `start_transfers`, but the items wait as held.
    pub(crate) fn enqueue_held(&mut self, direction: Direction, names: &[String]) {
        self.queue_transfers(direction, names, true);
    }

    fn queue_transfers(&mut self, direction: Direction, names: &[String], held: bool) {
        let (Some(live), Some(remote)) = (&self.live, &self.state.remote) else {
            self.notice(
                Level::Warning,
                "Connect to a server before transferring files.".to_owned(),
            );
            return;
        };
        let (source_entries, source_name) = match direction {
            Direction::Upload => (&self.state.local.entries, "local"),
            Direction::Download => (&remote.entries, "remote"),
        };
        let mut transfers = Vec::new();
        for name in names {
            let Some(entry) = source_entries.iter().find(|e| &e.name == name) else {
                tracing::debug!(target: "filecargo::app", %name, pane = source_name, "ignoring a name that is not in the pane");
                continue;
            };
            let is_dir = match entry.kind {
                EntryKind::Dir => true,
                EntryKind::File => false,
                _ => continue, // symlinks and special files are not transferred in v1
            };
            let Ok(remote_path) = remote.path.join(name) else {
                continue;
            };
            transfers.push(NewTransfer {
                site: live.site,
                direction,
                local: self.state.local.path.join(name),
                remote: remote_path,
                is_dir,
                size: (!is_dir).then_some(entry.size),
                conflict: None,
            });
        }
        if transfers.is_empty() {
            return;
        }
        if let Some(queue) = &self.queue {
            let _span = site_span(live.site).entered();
            tracing::info!(target: "filecargo::app", count = transfers.len(), ?direction, held, "queueing transfers");
            if held {
                queue.enqueue_held(transfers);
            } else {
                queue.enqueue(transfers);
            }
        }
    }

    pub(crate) fn queue_command(&mut self, command: Command) {
        let Some(queue) = self.queue.clone() else {
            return;
        };
        match command {
            Command::QueueRetry(id) => queue.retry(id),
            Command::QueueRetryFailed => queue.retry_all_failed(),
            Command::QueueRemove(id) => queue.remove(id),
            Command::QueueSetProcessing(on) => queue.set_processing(on),
            command => {
                // the rest act on the connected site
                let Some(site) = self.live.as_ref().map(|live| live.site) else {
                    self.notice(
                        Level::Warning,
                        "Connect to a server before using its queue.".to_owned(),
                    );
                    return;
                };
                match command {
                    Command::QueueClearCompleted => queue.clear_completed_site(site),
                    Command::QueueClearFailed => queue.clear_failed(site),
                    Command::QueueStartHeld => queue.start_held(site),
                    Command::QueueSetSitePaused(paused) => queue.set_site_paused(site, paused),
                    Command::QueueClear => self.ask_to_clear(site),
                    _ => {}
                }
            }
        }
    }

    /// `QueueClear`: asks first, unless there is nothing to clear.
    fn ask_to_clear(&mut self, site: SiteId) {
        let mine = self
            .state
            .queue
            .pending
            .iter()
            .filter(|v| v.item.site == site);
        let (items, active) = mine.fold((0, 0), |(items, active), view| {
            let running = matches!(view.item.state, ItemState::Active { .. });
            (items + 1, active + usize::from(running))
        });
        if items == 0 {
            return;
        }
        let id = self.enqueue_prompt(PromptKind::ConfirmClearQueue { items, active });
        self.actions.insert(id, PromptAction::ClearQueue { site });
    }

    /// The owner's answer to a conflict question. Anything but a decision skips the file.
    pub(crate) fn answer_conflict(&mut self, transfer: TransferId, answer: &PromptAnswer) {
        let Some(queue) = &self.queue else { return };
        let decision = match answer {
            PromptAnswer::Conflict(decision) => *decision,
            _ => ConflictDecision {
                rule: filecargo_config::ConflictRule::Skip,
                apply_to_all: false,
            },
        };
        queue.resolve(transfer, decision);
    }
}
