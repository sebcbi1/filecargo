//! The transfer queue: scheduling, conflict rules, resume, retry and persistence.

mod error;
mod model;
mod queue;
mod scheduler;
pub mod store;
mod worker;

pub use error::TransferError;
pub use model::{ConflictInfo, Direction, ItemState, NewTransfer, Outcome, QueueItem, TransferId};
pub use queue::{
    ConflictDecision, Connector, Queue, QueueEvent, QueueItemView, QueueLimits, QueueSnapshot,
    Totals,
};
