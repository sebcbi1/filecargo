//! The transfer queue: scheduling, conflict rules, resume, retry and persistence.

mod error;
mod model;
pub mod store;

pub use error::TransferError;
pub use model::{ConflictInfo, Direction, ItemState, NewTransfer, Outcome, QueueItem, TransferId};
