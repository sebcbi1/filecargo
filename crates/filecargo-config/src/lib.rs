//! Server tree, settings, secrets and FileZilla import, shared by the TUI and GUI.

mod error;
mod fsio;
mod model;
mod paths;
mod store;
mod tree;

pub use error::{ConfigError, ValidationError};
pub use model::{Auth, Folder, FolderId, FtpMode, NodeId, Protocol, Site, SiteId};
pub use paths::Paths;
pub use store::ConfigStore;
pub use tree::{Node, ServerTree, TreeOp};
