//! The single task that owns the queue. Workers run elsewhere and report back through a
//! channel, so no lock guards any queue state and every invariant lives in this file.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use filecargo_remote_fs::RemoteFs;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::queue::{
    ConflictDecision, Connector, QueueEvent, QueueItemView, QueueLimits, QueueSnapshot, Shared,
    Totals,
};
use crate::worker::{self, Child, Job, JobResult, ProgressCell};
use crate::{ItemState, NewTransfer, Outcome, QueueItem, TransferError, TransferId, store};
use filecargo_config::ConflictRule;

const COMPLETED_CAP: usize = 1_000;
const IDLE_CONNECTION: Duration = Duration::from_secs(30);
const EVENT_INTERVAL: Duration = Duration::from_millis(100);
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) enum Command {
    Enqueue(Vec<(TransferId, NewTransfer)>),
    Resolve(TransferId, ConflictDecision),
    Retry(TransferId),
    RetryAllFailed,
    Remove(TransferId),
    ClearCompleted,
    SetProcessing(bool),
    SetLimits(QueueLimits),
    Shutdown(oneshot::Sender<()>),
}

pub(crate) struct Init {
    pub connector: Arc<dyn Connector>,
    pub store_path: std::path::PathBuf,
    pub limits: QueueLimits,
    pub loaded: Vec<QueueItem>,
    pub shared: Arc<Shared>,
    pub commands: mpsc::UnboundedReceiver<Command>,
    pub events: mpsc::UnboundedSender<QueueEvent>,
    pub snapshot: watch::Sender<Arc<QueueSnapshot>>,
}

pub(crate) fn spawn(init: Init) {
    tokio::spawn(Scheduler::new(init).run());
}

/// A queue entry plus what only the scheduler knows about it.
struct Slot {
    item: QueueItem,
    /// Not before this instant (automatic retry back-off).
    retry_at: Option<Instant>,
    /// The owner's answer to this item's conflict.
    decided: Option<ConflictRule>,
}

impl Slot {
    fn new(item: QueueItem) -> Self {
        Self {
            item,
            retry_at: None,
            decided: None,
        }
    }
}

struct Running {
    handle: AbortHandle,
    progress: Arc<ProgressCell>,
}

struct Finished {
    id: TransferId,
    result: JobResult,
    /// The connection the job used, when it is still worth keeping.
    conn: Option<Arc<dyn RemoteFs>>,
}

struct Idle {
    site: filecargo_config::SiteId,
    fs: Arc<dyn RemoteFs>,
    since: Instant,
}

struct Scheduler {
    connector: Arc<dyn Connector>,
    store_path: std::path::PathBuf,
    limits: QueueLimits,
    processing: bool,
    shared: Arc<Shared>,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<QueueEvent>,
    snapshot: watch::Sender<Arc<QueueSnapshot>>,
    done_tx: mpsc::UnboundedSender<Finished>,
    done_rx: mpsc::UnboundedReceiver<Finished>,

    pending: Vec<Slot>,
    failed: Vec<Slot>,
    /// Newest first.
    completed: VecDeque<QueueItem>,
    active: HashMap<TransferId, Running>,
    pool: Vec<Idle>,
    /// An `apply_to_all` answer: used instead of asking until the queue is empty.
    override_rule: Option<ConflictRule>,

    changed: bool,
    last_event: Instant,
    persist_dirty: bool,
    last_save: Instant,
    was_idle: bool,
}

impl Scheduler {
    fn new(init: Init) -> Self {
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let now = Instant::now();
        let (mut pending, mut failed) = (Vec::new(), Vec::new());
        for item in init.loaded {
            match item.state {
                ItemState::Failed { .. } => failed.push(Slot::new(item)),
                _ => pending.push(Slot::new(item)),
            }
        }
        // a freshly started empty queue is idle already: no `Idle` event for that
        let pending_is_empty = pending.is_empty();
        Self {
            connector: init.connector,
            store_path: init.store_path,
            limits: init.limits,
            processing: true,
            shared: init.shared,
            commands: init.commands,
            events: init.events,
            snapshot: init.snapshot,
            done_tx,
            done_rx,
            pending,
            failed,
            completed: VecDeque::new(),
            active: HashMap::new(),
            pool: Vec::new(),
            override_rule: None,
            changed: true,
            last_event: now.checked_sub(EVENT_INTERVAL).unwrap_or(now),
            persist_dirty: false,
            last_save: now,
            was_idle: pending_is_empty,
        }
    }

    async fn run(mut self) {
        self.start_ready();
        loop {
            let wake = self.next_wake();
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => {
                        if self.handle(command).await {
                            return;
                        }
                    }
                    None => {
                        self.stop_workers_and_save();
                        return;
                    }
                },
                Some(finished) = self.done_rx.recv() => self.on_finished(finished),
                () = tokio::time::sleep_until(wake) => {}
            }
            self.housekeeping();
        }
    }

    // ---- commands ---------------------------------------------------------------------

    /// Returns `true` when the scheduler should stop.
    async fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Enqueue(items) => self.enqueue(items),
            Command::Resolve(id, decision) => self.resolve(id, decision),
            Command::Retry(id) => self.retry(id),
            Command::RetryAllFailed => {
                let ids: Vec<_> = self.failed.iter().map(|s| s.item.id).collect();
                ids.into_iter().for_each(|id| self.retry(id));
            }
            Command::Remove(id) => self.remove(id),
            Command::ClearCompleted => {
                self.completed.clear();
                self.touch(false);
            }
            Command::SetProcessing(on) => {
                self.processing = on;
                self.touch(false);
            }
            Command::SetLimits(limits) => {
                self.limits = limits;
                self.touch(false);
            }
            Command::Shutdown(ack) => {
                self.stop_workers_and_save();
                let _ = ack.send(());
                return true;
            }
        }
        false
    }

    fn enqueue(&mut self, items: Vec<(TransferId, NewTransfer)>) {
        for (id, new) in items {
            self.pending.push(Slot::new(QueueItem {
                id,
                site: new.site,
                direction: new.direction,
                local: new.local,
                remote: new.remote,
                is_dir: new.is_dir,
                size: new.size,
                transferred: 0,
                state: ItemState::Pending,
                attempts: 0,
                parent: None,
                conflict: new.conflict,
            }));
        }
        self.was_idle = false;
        self.touch(true);
    }

    fn resolve(&mut self, id: TransferId, decision: ConflictDecision) {
        if decision.rule == ConflictRule::Ask {
            tracing::warn!(target: "filecargo::transfer", %id, "ignoring a conflict answer of `ask`");
            return;
        }
        let waiting = |s: &Slot| matches!(s.item.state, ItemState::AwaitingDecision { .. });
        if decision.apply_to_all {
            self.override_rule = Some(decision.rule);
            for slot in self.pending.iter_mut().filter(|s| waiting(s)) {
                slot.decided = Some(decision.rule);
                slot.item.state = ItemState::Pending;
            }
        } else if let Some(slot) = self
            .pending
            .iter_mut()
            .find(|s| s.item.id == id && waiting(s))
        {
            slot.decided = Some(decision.rule);
            slot.item.state = ItemState::Pending;
        }
        self.touch(true);
    }

    fn retry(&mut self, id: TransferId) {
        let Some(index) = self.failed.iter().position(|s| s.item.id == id) else {
            return;
        };
        let mut slot = self.failed.remove(index);
        slot.item.attempts = 0;
        slot.item.state = ItemState::Pending;
        slot.retry_at = None;
        self.pending.push(slot);
        self.was_idle = false;
        self.touch(true);
    }

    fn remove(&mut self, id: TransferId) {
        if let Some(running) = self.active.remove(&id) {
            running.handle.abort();
        }
        self.pending.retain(|s| s.item.id != id);
        self.failed.retain(|s| s.item.id != id);
        self.completed.retain(|i| i.id != id);
        self.touch(true);
    }

    // ---- scheduling -----------------------------------------------------------------------

    /// The rule for an item: the owner's answer, else the item's own, else the queue default;
    /// an `Ask` becomes the `apply_to_all` answer when there is one.
    fn effective_rule(&self, slot: &Slot) -> ConflictRule {
        let base = slot
            .decided
            .or(slot.item.conflict)
            .unwrap_or(self.limits.default_conflict);
        if base == ConflictRule::Ask {
            self.override_rule.unwrap_or(ConflictRule::Ask)
        } else {
            base
        }
    }

    fn start_ready(&mut self) {
        if !self.processing {
            return;
        }
        let now = Instant::now();
        while self.active.len() < usize::from(self.limits.max_concurrent.max(1)) {
            let Some(index) = self.pending.iter().position(|s| {
                matches!(s.item.state, ItemState::Pending) && s.retry_at.is_none_or(|at| at <= now)
            }) else {
                break;
            };
            self.start(index);
        }
    }

    fn start(&mut self, index: usize) {
        let rule = self.effective_rule(&self.pending[index]);
        let slot = &mut self.pending[index];
        slot.retry_at = None;
        slot.item.state = ItemState::Active {
            started: SystemTime::now(),
        };
        let item = slot.item.clone();
        let id = item.id;
        let progress = Arc::new(ProgressCell::default());
        progress.start_at(item.transferred);

        let pooled = self
            .pool
            .iter()
            .position(|c| c.site == item.site)
            .map(|i| self.pool.swap_remove(i).fs);
        let connector = self.connector.clone();
        let done = self.done_tx.clone();
        let job_progress = progress.clone();
        let handle = tokio::spawn(async move {
            let fs = match pooled {
                Some(fs) => fs,
                None => match connector.connect(item.site).await {
                    Ok(fs) => fs,
                    Err(error) => {
                        let _ = done.send(Finished {
                            id,
                            result: JobResult::Failed(error),
                            conn: None,
                        });
                        return;
                    }
                },
            };
            let result = worker::run(Job {
                item,
                rule,
                fs: fs.clone(),
                progress: job_progress,
            })
            .await;
            let _ = done.send(Finished {
                id,
                result,
                conn: Some(fs),
            });
        });
        self.active.insert(
            id,
            Running {
                handle: handle.abort_handle(),
                progress,
            },
        );
        self.touch(true);
    }

    fn on_finished(&mut self, finished: Finished) {
        let Finished { id, result, conn } = finished;
        let Some(running) = self.active.remove(&id) else {
            // cancelled while its result was in flight
            return;
        };
        let Some(index) = self.pending.iter().position(|s| s.item.id == id) else {
            return;
        };
        let transferred = running.progress.transferred();
        self.pending[index].item.transferred = transferred;
        if let Some(retarget) = running.progress.take_retarget() {
            self.pending[index].item.local = retarget.local;
            self.pending[index].item.remote = retarget.remote;
        }

        let site = self.pending[index].item.site;
        let keep = |this: &mut Self, conn: Option<Arc<dyn RemoteFs>>| {
            if let Some(fs) = conn {
                this.pool.push(Idle {
                    site,
                    fs,
                    since: Instant::now(),
                });
            }
        };
        match result {
            JobResult::Completed(outcome) => {
                keep(self, conn);
                self.complete(index, outcome);
            }
            JobResult::NeedsDecision(conflict) => {
                keep(self, conn);
                if let Some(rule) = self.override_rule {
                    // The owner answered "apply to all" while this item was already running:
                    // use that answer, do not ask again.
                    let slot = &mut self.pending[index];
                    slot.decided = Some(rule);
                    slot.item.state = ItemState::Pending;
                } else {
                    self.pending[index].item.state = ItemState::AwaitingDecision {
                        conflict: conflict.clone(),
                    };
                    // events and snapshot must agree for a question the owner will act on
                    self.publish_snapshot();
                    let _ = self.events.send(QueueEvent::ConflictAsked { id, conflict });
                }
            }
            JobResult::Expanded(children) => {
                keep(self, conn);
                self.expand(index, children);
                self.complete(index, Outcome::Created);
            }
            JobResult::Failed(error) => {
                self.close_in_background(conn);
                self.fail(index, error);
            }
        }
        self.touch(true);
    }

    /// Puts a directory's children right after it, in listing order.
    fn expand(&mut self, index: usize, children: Vec<Child>) {
        let parent = self.pending[index].item.clone();
        let slots: Vec<Slot> = children
            .into_iter()
            .map(|child| {
                let id = TransferId(
                    self.shared
                        .next_id
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                );
                Slot::new(QueueItem {
                    id,
                    site: parent.site,
                    direction: parent.direction,
                    local: child.local,
                    remote: child.remote,
                    is_dir: child.is_dir,
                    size: child.size,
                    transferred: 0,
                    state: ItemState::Pending,
                    attempts: 0,
                    parent: Some(parent.id),
                    conflict: parent.conflict,
                })
            })
            .collect();
        self.pending.splice(index + 1..index + 1, slots);
    }

    fn complete(&mut self, index: usize, outcome: Outcome) {
        let mut slot = self.pending.remove(index);
        slot.item.state = ItemState::Completed {
            outcome,
            finished: SystemTime::now(),
        };
        let id = slot.item.id;
        self.completed.push_front(slot.item);
        self.completed.truncate(COMPLETED_CAP);
        let _ = self.events.send(QueueEvent::ItemFinished { id, ok: true });
    }

    fn fail(&mut self, index: usize, error: TransferError) {
        let mut slot = self.pending.remove(index);
        slot.item.attempts += 1;
        slot.item.state = ItemState::Failed {
            reason: error.to_string(),
            retryable: error.is_retryable(),
            finished: SystemTime::now(),
        };
        let id = slot.item.id;
        self.failed.push(slot);
        let _ = self.events.send(QueueEvent::ItemFinished { id, ok: false });
    }

    fn close_in_background(&self, conn: Option<Arc<dyn RemoteFs>>) {
        if let Some(fs) = conn {
            tokio::spawn(async move { fs.close().await });
        }
    }

    // ---- bookkeeping ----------------------------------------------------------------------------

    /// Marks the view as changed; `persist` when persisted data changed too.
    fn touch(&mut self, persist: bool) {
        self.changed = true;
        self.persist_dirty |= persist;
        self.start_ready();
    }

    fn housekeeping(&mut self) {
        let now = Instant::now();
        // progress of running items
        for slot in &mut self.pending {
            if let Some(running) = self.active.get(&slot.item.id) {
                let transferred = running.progress.transferred();
                if transferred != slot.item.transferred {
                    slot.item.transferred = transferred;
                    self.changed = true;
                }
            }
        }
        // idle connections
        let (keep, expired): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pool)
            .into_iter()
            .partition(|c| now.duration_since(c.since) < IDLE_CONNECTION);
        self.pool = keep;
        for idle in expired {
            self.close_in_background(Some(idle.fs));
        }
        self.start_ready();

        let idle_now = self.pending.is_empty() && self.active.is_empty();
        if idle_now && !self.was_idle {
            self.was_idle = true;
            self.override_rule = None;
            self.persist_dirty = true;
            self.publish();
            let _ = self.events.send(QueueEvent::Idle);
        } else if !idle_now {
            self.was_idle = false;
        }

        if self.persist_dirty && now.duration_since(self.last_save) >= SAVE_INTERVAL {
            self.save();
        }
        if self.changed && now.duration_since(self.last_event) >= EVENT_INTERVAL {
            self.publish();
            let _ = self.events.send(QueueEvent::Changed);
        }
    }

    fn next_wake(&self) -> Instant {
        let now = Instant::now();
        let mut wake = now + Duration::from_secs(3600);
        let mut consider = |at: Instant| wake = wake.min(at.max(now));
        if self.changed {
            consider(self.last_event + EVENT_INTERVAL);
        }
        if !self.active.is_empty() {
            consider(now + EVENT_INTERVAL);
        }
        if self.persist_dirty {
            consider(self.last_save + SAVE_INTERVAL);
        }
        for slot in &self.pending {
            if let Some(at) = slot.retry_at {
                consider(at);
            }
        }
        for idle in &self.pool {
            consider(idle.since + IDLE_CONNECTION);
        }
        wake
    }

    /// Publishes the snapshot and counts as the `Changed` notification.
    fn publish(&mut self) {
        self.changed = false;
        self.last_event = Instant::now();
        self.publish_snapshot();
    }

    /// Publishes the snapshot only; a `Changed` event still follows within 100 ms.
    fn publish_snapshot(&mut self) {
        let view = |item: &QueueItem| QueueItemView {
            item: item.clone(),
            rate: None,
            eta: None,
        };
        let snapshot = QueueSnapshot {
            pending: self.pending.iter().map(|s| view(&s.item)).collect(),
            completed: self.completed.iter().map(view).collect(),
            failed: self.failed.iter().map(|s| view(&s.item)).collect(),
            processing: self.processing,
            totals: Totals::default(),
        };
        let _ = self.snapshot.send(Arc::new(snapshot));
    }

    fn save(&mut self) {
        self.persist_dirty = false;
        self.last_save = Instant::now();
        let items: Vec<QueueItem> = self
            .pending
            .iter()
            .chain(&self.failed)
            .map(|s| s.item.clone())
            .collect();
        let next_id = self
            .shared
            .next_id
            .load(std::sync::atomic::Ordering::SeqCst);
        if let Err(error) = store::save(&self.store_path, &items, next_id) {
            tracing::warn!(target: "filecargo::transfer", %error, "could not save the queue");
        }
    }

    fn stop_workers_and_save(&mut self) {
        for slot in &mut self.pending {
            if let Some(running) = self.active.remove(&slot.item.id) {
                running.handle.abort();
                slot.item.transferred = running.progress.transferred();
                slot.item.state = ItemState::Pending;
            }
        }
        self.active.clear();
        self.save();
        for idle in std::mem::take(&mut self.pool) {
            self.close_in_background(Some(idle.fs));
        }
    }
}
