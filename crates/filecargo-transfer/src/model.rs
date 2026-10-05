use std::path::PathBuf;
use std::time::SystemTime;

use filecargo_config::{ConflictRule, SiteId};
use filecargo_remote_fs::{Entry, RemotePath};
use serde::{Deserialize, Serialize};

/// Monotonic and unique within one `queue.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(pub u64);

impl std::fmt::Display for TransferId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTransfer {
    pub site: SiteId,
    pub direction: Direction,
    /// File or directory on this machine.
    pub local: PathBuf,
    /// File or directory on the server.
    pub remote: RemotePath,
    pub is_dir: bool,
    /// Known size of the source, for progress before it is stat'ed.
    pub size: Option<u64>,
    /// `None` means the queue's default rule.
    pub conflict: Option<ConflictRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub id: TransferId,
    pub site: SiteId,
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: RemotePath,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Bytes we have written to the target: the resume offset.
    pub transferred: u64,
    pub state: ItemState,
    pub attempts: u32,
    /// The directory item that produced this one.
    pub parent: Option<TransferId>,
    /// Per-item rule; `None` falls back to the queue's rule.
    pub conflict: Option<ConflictRule>,
}

// `AwaitingDecision` carries two directory entries; items live in a queue of a few thousand at
// most, so the size difference between variants does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemState {
    Pending,
    Active {
        started: SystemTime,
    },
    /// Waiting for the owner to answer a conflict.
    AwaitingDecision {
        conflict: ConflictInfo,
    },
    Completed {
        outcome: Outcome,
        finished: SystemTime,
    },
    Failed {
        reason: String,
        retryable: bool,
        finished: SystemTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Transferred,
    Skipped,
    Resumed,
    Renamed(String),
    /// A directory was created (or merged into).
    Created,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictInfo {
    pub source: Entry,
    pub target: Entry,
}
