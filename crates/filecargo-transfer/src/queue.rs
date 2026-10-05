//! The public face of the queue: the handle, the traits it needs, and what it reports.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use filecargo_config::{ConflictRule, SiteId};
use filecargo_remote_fs::RemoteFs;
use tokio::sync::{mpsc, oneshot, watch};

use crate::scheduler::{self, Command};
use crate::{ConflictInfo, NewTransfer, QueueItem, TransferError, TransferId, store};

/// Opens connections for workers. Implemented by `app-core` (the real `remote_fs::connect`
/// with the shared `SessionTrust`) and by tests (a `RootedFs`, or a fault-injecting wrapper).
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, site: SiteId) -> Result<Arc<dyn RemoteFs>, TransferError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueLimits {
    pub max_concurrent: u8,
    pub default_conflict: ConflictRule,
}

impl Default for QueueLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            default_conflict: ConflictRule::Ask,
        }
    }
}

/// The owner's answer to a [`QueueEvent::ConflictAsked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictDecision {
    /// Never `Ask`.
    pub rule: ConflictRule,
    /// Also answers every later conflict (and the ones already waiting) until the queue is
    /// empty.
    pub apply_to_all: bool,
}

// Events are transient and few; the size of `ConflictAsked` does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    /// `snapshot()` has new content. Coalesced: at most 10 per second.
    Changed,
    ConflictAsked {
        id: TransferId,
        conflict: ConflictInfo,
    },
    /// For notifications and log lines.
    ItemFinished { id: TransferId, ok: bool },
    /// Nothing pending or active.
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueItemView {
    pub item: QueueItem,
    /// Bytes per second, averaged over about 5 s. Only for active items.
    pub rate: Option<f64>,
    /// Hidden until a second of data exists.
    pub eta: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub rate: Option<f64>,
    pub eta: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueSnapshot {
    /// Including `Active` and `AwaitingDecision`, in queue order.
    pub pending: Vec<QueueItemView>,
    /// Newest first, capped at 1,000.
    pub completed: Vec<QueueItemView>,
    pub failed: Vec<QueueItemView>,
    pub processing: bool,
    pub totals: Totals,
}

pub(crate) struct Shared {
    pub(crate) commands: mpsc::UnboundedSender<Command>,
    pub(crate) next_id: AtomicU64,
    pub(crate) snapshot: watch::Receiver<Arc<QueueSnapshot>>,
}

/// A cheap-to-clone handle; the scheduler runs as a tokio task.
#[derive(Clone)]
pub struct Queue {
    shared: Arc<Shared>,
}

impl Queue {
    /// Loads `queue.json` (if any) and starts the scheduler, processing.
    pub async fn start(
        connector: Arc<dyn Connector>,
        store_path: PathBuf,
        limits: QueueLimits,
    ) -> Result<(Queue, mpsc::UnboundedReceiver<QueueEvent>), TransferError> {
        let loaded = store::load(&store_path);
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(QueueSnapshot {
            processing: true,
            ..QueueSnapshot::default()
        }));
        let shared = Arc::new(Shared {
            commands: commands_tx,
            next_id: AtomicU64::new(loaded.next_id),
            snapshot: snapshot_rx,
        });
        scheduler::spawn(scheduler::Init {
            connector,
            store_path,
            limits,
            loaded: loaded.items,
            shared: shared.clone(),
            commands: commands_rx,
            events: events_tx,
            snapshot: snapshot_tx,
        });
        Ok((Queue { shared }, events_rx))
    }

    fn send(&self, command: Command) {
        // The scheduler only goes away after `shutdown`; later calls are no-ops.
        let _ = self.shared.commands.send(command);
    }

    pub fn enqueue(&self, items: Vec<NewTransfer>) -> Vec<TransferId> {
        let ids: Vec<TransferId> = items
            .iter()
            .map(|_| TransferId(self.shared.next_id.fetch_add(1, Ordering::SeqCst)))
            .collect();
        self.send(Command::Enqueue(ids.iter().copied().zip(items).collect()));
        ids
    }

    /// Answers a [`QueueEvent::ConflictAsked`].
    pub fn resolve(&self, id: TransferId, decision: ConflictDecision) {
        self.send(Command::Resolve(id, decision));
    }

    pub fn retry(&self, id: TransferId) {
        self.send(Command::Retry(id));
    }

    pub fn retry_all_failed(&self) {
        self.send(Command::RetryAllFailed);
    }

    /// Cancels an active item; a partial file is left as is.
    pub fn remove(&self, id: TransferId) {
        self.send(Command::Remove(id));
    }

    pub fn clear_completed(&self) {
        self.send(Command::ClearCompleted);
    }

    /// Pauses or resumes *starting* new items; running ones finish.
    pub fn set_processing(&self, on: bool) {
        self.send(Command::SetProcessing(on));
    }

    pub fn set_limits(&self, limits: QueueLimits) {
        self.send(Command::SetLimits(limits));
    }

    /// The latest published view; cheap.
    pub fn snapshot(&self) -> Arc<QueueSnapshot> {
        self.shared.snapshot.borrow().clone()
    }

    /// Stops the workers (active items return to pending with their offset) and writes
    /// `queue.json`.
    pub async fn shutdown(self) {
        let (done_tx, done_rx) = oneshot::channel();
        self.send(Command::Shutdown(done_tx));
        let _ = done_rx.await;
    }
}
