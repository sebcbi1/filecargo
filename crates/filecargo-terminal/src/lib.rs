//! Terminal emulation and key encoding for the shell tab of SFTP sessions.

mod handle;
mod keys;

pub use handle::{TermSize, TermStatus, TerminalHandle, spawn};
pub use keys::{Key, Modes, Mods, encode};
pub use vt100::Screen;
