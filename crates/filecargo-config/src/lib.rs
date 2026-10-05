//! Server tree, settings, secrets and FileZilla import, shared by the TUI and GUI.

mod error;
mod paths;

pub use error::ConfigError;
pub use paths::Paths;
