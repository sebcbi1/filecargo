use std::time::SystemTime;

use async_trait::async_trait;
use filecargo_config::Protocol;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{Capabilities, Entry, EntryKind, FsError, RemotePath};

/// Receives the running byte total of a transfer.
pub trait Progress: Send + Sync {
    fn advance(&self, total_bytes_so_far: u64);
}

/// A [`Progress`] that ignores everything.
pub struct NoProgress;

impl Progress for NoProgress {
    fn advance(&self, _total_bytes_so_far: u64) {}
}

/// One async filesystem interface over SFTP, FTP/FTPS and the local disk.
///
/// Rules every backend follows:
/// - Cancellation is **dropping the future**. A backend whose connection cannot be trusted
///   after a cancelled transfer marks itself broken: later calls return
///   [`FsError::Disconnected`].
/// - Calls may come from several tasks; backends serialize internally when the protocol needs it.
/// - `list` returns entries in server order. Sorting and hiding dotfiles is the caller's job.
#[async_trait]
pub trait RemoteFs: Send + Sync {
    fn protocol(&self) -> Protocol;
    fn capabilities(&self) -> Capabilities;
    /// Directory the server starts in (SFTP `canonicalize(".")`, FTP `PWD`).
    async fn home(&self) -> Result<RemotePath, FsError>;
    /// Entries of `dir`, excluding `.` and `..`.
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError>;
    /// `Ok(None)` when nothing exists at `path`. Symlinks are reported, not followed.
    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError>;
    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError>;
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError>;
    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError>;
    /// The directory must be empty.
    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError>;
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError>;
    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError>;

    /// Streams `path` from byte `offset` into `sink`; returns the bytes written.
    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError>;

    /// Writes `source` to `path`. `offset == 0` truncates or creates; `offset > 0` resumes (the
    /// remote file must currently be exactly `offset` bytes long). Returns the bytes read.
    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError>;

    /// Recursive delete built on `list` / `remove_file` / `remove_dir`. Symlinks are removed,
    /// never followed.
    async fn remove_all(&self, path: &RemotePath) -> Result<(), FsError> {
        let entry = self
            .stat(path)
            .await?
            .ok_or_else(|| FsError::NotFound(path.to_string()))?;
        if entry.kind != EntryKind::Dir {
            return self.remove_file(path).await;
        }
        for child in self.list(path).await? {
            let child_path = path.join(&child.name).map_err(|e| FsError::Protocol {
                code: None,
                message: e.to_string(),
            })?;
            if child.kind == EntryKind::Dir {
                // An empty directory goes with one cheap call; only a non-empty one (or a real
                // failure, which then resurfaces) needs listing. On FTP every listing opens a
                // data connection.
                if self.remove_dir(&child_path).await.is_err() {
                    self.remove_all(&child_path).await?;
                }
            } else {
                self.remove_file(&child_path).await?;
            }
        }
        self.remove_dir(path).await
    }

    /// Best-effort clean shutdown (FTP `QUIT`, SSH disconnect).
    async fn close(&self);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    /// Minimal tree fake: only what `remove_all` touches is implemented.
    struct MemFs {
        nodes: Mutex<BTreeMap<String, EntryKind>>,
    }

    impl MemFs {
        fn new(nodes: &[(&str, EntryKind)]) -> Self {
            let mut map = BTreeMap::new();
            map.insert("/".to_owned(), EntryKind::Dir);
            map.extend(nodes.iter().map(|(p, k)| ((*p).to_owned(), k.clone())));
            Self {
                nodes: Mutex::new(map),
            }
        }

        fn paths(&self) -> Vec<String> {
            self.nodes.lock().unwrap().keys().cloned().collect()
        }
    }

    #[async_trait]
    impl RemoteFs for MemFs {
        fn protocol(&self) -> Protocol {
            Protocol::Sftp
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                chmod: false,
                set_modified: false,
                resume_upload: false,
                resume_download: false,
            }
        }
        async fn home(&self) -> Result<RemotePath, FsError> {
            Ok(RemotePath::root())
        }
        async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
            let nodes = self.nodes.lock().unwrap();
            Ok(nodes
                .iter()
                .filter(|(p, _)| p.as_str() != "/")
                .filter_map(|(p, k)| {
                    let path = RemotePath::parse(p).ok()?;
                    (path.parent().as_ref() == Some(dir))
                        .then(|| Entry::new(path.file_name().unwrap_or_default(), k.clone()))
                })
                .collect())
        }
        async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
            Ok(self
                .nodes
                .lock()
                .unwrap()
                .get(path.as_str())
                .map(|k| Entry::new(path.file_name().unwrap_or("/"), k.clone())))
        }
        async fn mkdir(&self, _: &RemotePath) -> Result<(), FsError> {
            Err(FsError::Unsupported("mkdir"))
        }
        async fn rename(&self, _: &RemotePath, _: &RemotePath) -> Result<(), FsError> {
            Err(FsError::Unsupported("rename"))
        }
        async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
            self.nodes.lock().unwrap().remove(path.as_str());
            Ok(())
        }
        async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
            if !self.list(path).await?.is_empty() {
                return Err(FsError::DirectoryNotEmpty(path.to_string()));
            }
            self.nodes.lock().unwrap().remove(path.as_str());
            Ok(())
        }
        async fn chmod(&self, _: &RemotePath, _: u32) -> Result<(), FsError> {
            Err(FsError::Unsupported("chmod"))
        }
        async fn set_modified(&self, _: &RemotePath, _: SystemTime) -> Result<(), FsError> {
            Err(FsError::Unsupported("set_modified"))
        }
        async fn download(
            &self,
            _: &RemotePath,
            _: u64,
            _: &mut (dyn AsyncWrite + Send + Unpin),
            _: &dyn Progress,
        ) -> Result<u64, FsError> {
            Err(FsError::Unsupported("download"))
        }
        async fn upload(
            &self,
            _: &RemotePath,
            _: u64,
            _: &mut (dyn AsyncRead + Send + Unpin),
            _: &dyn Progress,
        ) -> Result<u64, FsError> {
            Err(FsError::Unsupported("upload"))
        }
        async fn close(&self) {}
    }

    fn p(s: &str) -> RemotePath {
        RemotePath::parse(s).unwrap()
    }

    #[tokio::test]
    async fn remove_all_deletes_a_tree_but_not_its_siblings() {
        let fs = MemFs::new(&[
            ("/keep", EntryKind::File),
            ("/d", EntryKind::Dir),
            ("/d/a", EntryKind::File),
            ("/d/sub", EntryKind::Dir),
            ("/d/sub/b", EntryKind::File),
            (
                "/d/link",
                EntryKind::Symlink {
                    target: Some("/keep".into()),
                },
            ),
        ]);
        fs.remove_all(&p("/d")).await.unwrap();
        assert_eq!(fs.paths(), ["/", "/keep"]);
    }

    #[tokio::test]
    async fn remove_all_on_a_file_removes_it() {
        let fs = MemFs::new(&[("/f", EntryKind::File)]);
        fs.remove_all(&p("/f")).await.unwrap();
        assert_eq!(fs.paths(), ["/"]);
    }

    #[tokio::test]
    async fn remove_all_on_a_missing_path_is_not_found() {
        let fs = MemFs::new(&[]);
        assert_eq!(
            fs.remove_all(&p("/nope")).await,
            Err(FsError::NotFound("/nope".into()))
        );
    }
}
