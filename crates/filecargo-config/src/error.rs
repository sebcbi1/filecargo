use std::path::PathBuf;

use crate::model::FolderId;

/// Errors returned by the config module.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot determine home directory: {0}")]
    NoHomeDir(String),
    #[error("i/o error on {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}: {msg}")]
    Parse {
        path: PathBuf,
        line: usize,
        msg: String,
    },
    #[error(
        "{path} was written by a newer filecargo (file version {found}, supported {supported})"
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("cannot serialize config: {0}")]
    Serialize(String),
    #[error("invalid server tree: {0}")]
    Invalid(#[from] ValidationError),
}

/// A rule of the server tree that an operation or a loaded file violates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("host must not be empty (site \"{site}\")")]
    EmptyHost { site: String },
    #[error("port must be between 1 and 65535 (site \"{site}\")")]
    InvalidPort { site: String },
    #[error("{auth} authentication is not available for {protocol} (site \"{site}\")")]
    AuthProtocolMismatch {
        site: String,
        auth: &'static str,
        protocol: &'static str,
    },
    #[error("\"{name}\" already exists in this folder")]
    DuplicateName { name: String },
    #[error("unknown folder {0}")]
    UnknownFolder(FolderId),
    #[error("node not found")]
    UnknownNode,
    #[error("a folder cannot be moved into itself or one of its subfolders")]
    MoveIntoDescendant,
    #[error("duplicate id {0}")]
    DuplicateId(String),
}
