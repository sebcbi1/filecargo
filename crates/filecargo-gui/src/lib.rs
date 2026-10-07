//! The desktop front-end: gpui views over the snapshots and commands of `app-core`.
//! No I/O happens here (and no tokio inside gpui tasks): views send `Command`s and render the
//! latest `AppState`.

pub mod bottom;
pub mod dialogs;
pub mod format;
pub mod icon;
pub mod model;
pub mod notices;
pub mod pane;
pub mod prompts;
pub mod terminal;
pub mod toolbar;
pub mod tree;
pub mod workspace;
