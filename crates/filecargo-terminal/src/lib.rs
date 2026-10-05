//! Terminal emulation and key encoding for the shell tab of SFTP sessions.

mod keys;

pub use keys::{Key, Modes, Mods, encode};
