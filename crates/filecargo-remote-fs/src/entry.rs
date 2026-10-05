use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink { target: Option<String> },
    Other,
}

/// One directory entry. The same type serves local and remote listings; it carries no path,
/// callers join `name` onto the directory they listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// 0 for directories.
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Unix mode bits (`0o7777` mask) when known.
    pub permissions: Option<u32>,
    /// Usually `None`: SFTPv3 and most FTP listings carry no names.
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl Entry {
    pub fn new(name: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            name: name.into(),
            kind,
            size: 0,
            modified: None,
            permissions: None,
            owner: None,
            group: None,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}

/// What a backend can do beyond the mandatory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub chmod: bool,
    pub set_modified: bool,
    pub resume_upload: bool,
    pub resume_download: bool,
}
