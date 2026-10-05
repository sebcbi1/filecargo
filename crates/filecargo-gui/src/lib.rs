//! The desktop front-end: gpui views over the snapshots and commands of `app-core`.
//! No I/O happens here (and no tokio inside gpui tasks): views send `Command`s and render the
//! latest `AppState`.

pub mod format;
pub mod model;
pub mod pane;
pub mod toolbar;
pub mod tree;
pub mod workspace;
