use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity of a site; survives renames and moves, and keys its keychain secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiteId(Uuid);

impl SiteId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SiteId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderId(Uuid);

impl FolderId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for FolderId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for FolderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Either kind of node in the server tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeId {
    Site(SiteId),
    Folder(FolderId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Sftp,
    Ftp,
    FtpsExplicit,
    FtpsImplicit,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Self::Sftp => 22,
            Self::Ftp | Self::FtpsExplicit => 21,
            Self::FtpsImplicit => 990,
        }
    }

    pub fn is_ftp_family(self) -> bool {
        !matches!(self, Self::Sftp)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Auth {
    /// FTP/FTPS only.
    Anonymous,
    /// Also used for SSH keyboard-interactive.
    Password { remember: bool },
    /// SFTP only.
    KeyFile {
        path: PathBuf,
        remember_passphrase: bool,
    },
    /// SFTP only.
    Agent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtpMode {
    #[default]
    Passive,
    Active,
}

impl FtpMode {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub id: SiteId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderId>,
    pub protocol: Protocol,
    pub host: String,
    /// `None` means the protocol default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: String,
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "FtpMode::is_default")]
    pub ftp_mode: FtpMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl Site {
    /// A new root-level site with a fresh id, password auth (not remembered) and defaults.
    pub fn new(name: impl Into<String>, protocol: Protocol, host: impl Into<String>) -> Self {
        Self {
            id: SiteId::new(),
            name: name.into(),
            folder: None,
            protocol,
            host: host.into(),
            port: None,
            user: String::new(),
            auth: Auth::Password { remember: false },
            ftp_mode: FtpMode::default(),
            remote_dir: None,
            local_dir: None,
            notes: String::new(),
        }
    }

    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<FolderId>,
}
