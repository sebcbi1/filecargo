//! The local filesystem: listings for the local pane, and [`RootedFs`], a [`RemoteFs`] whose
//! root is a directory on disk (used by tests in place of a server, never for real sessions).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use filecargo_config::Protocol;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

use crate::{Capabilities, Entry, EntryKind, FsError, Progress, RemoteFs, RemotePath};

const CHUNK: usize = 64 * 1024;

fn map_io(err: std::io::Error, path: &Path) -> FsError {
    let what = path.display().to_string();
    match err.kind() {
        ErrorKind::NotFound => FsError::NotFound(what),
        ErrorKind::PermissionDenied => FsError::PermissionDenied(what),
        ErrorKind::AlreadyExists => FsError::AlreadyExists(what),
        ErrorKind::NotADirectory => FsError::NotADirectory(what),
        ErrorKind::DirectoryNotEmpty => FsError::DirectoryNotEmpty(what),
        _ => FsError::LocalIo(format!("{what}: {err}")),
    }
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> Option<u32> {
    None
}

async fn entry_from(name: String, path: &Path, meta: &std::fs::Metadata) -> Entry {
    let file_type = meta.file_type();
    let kind = if file_type.is_symlink() {
        let target = tokio::fs::read_link(path)
            .await
            .ok()
            .map(|t| t.to_string_lossy().into_owned());
        EntryKind::Symlink { target }
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Entry {
        name,
        size: if kind == EntryKind::Dir {
            0
        } else {
            meta.len()
        },
        kind,
        modified: meta.modified().ok(),
        permissions: mode_of(meta),
        owner: None,
        group: None,
    }
}

/// Listing for the local pane. Symlinks are reported, not followed.
pub async fn read_dir(dir: &Path) -> Result<Vec<Entry>, FsError> {
    let mut reader = tokio::fs::read_dir(dir).await.map_err(|e| map_io(e, dir))?;
    let mut entries = Vec::new();
    while let Some(item) = reader.next_entry().await.map_err(|e| map_io(e, dir))? {
        let path = item.path();
        let meta = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|e| map_io(e, &path))?;
        let name = item.file_name().to_string_lossy().into_owned();
        entries.push(entry_from(name, &path, &meta).await);
    }
    Ok(entries)
}

/// `Ok(None)` when nothing exists at `path`. A final symlink is reported, not followed.
pub async fn stat(path: &Path) -> Result<Option<Entry>, FsError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Ok(Some(entry_from(name, path, &meta).await))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(map_io(e, path)),
    }
}

/// A [`RemoteFs`] whose root `/` is a directory on disk. Paths never escape the root, not
/// even through symlinks.
#[derive(Debug)]
pub struct RootedFs {
    root: PathBuf,
}

impl RootedFs {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, FsError> {
        let root = root.as_ref();
        let root = std::fs::canonicalize(root).map_err(|e| map_io(e, root))?;
        Ok(Self { root })
    }

    /// Maps `path` under the root. With `follow`, a final symlink is resolved too (operations
    /// that open the file); either way an existing parent chain must stay inside the root.
    async fn resolve(&self, path: &RemotePath, follow: bool) -> Result<PathBuf, FsError> {
        let mut full = self.root.clone();
        full.extend(path.as_str().split('/').filter(|s| !s.is_empty()));
        if *path == RemotePath::root() {
            return Ok(full);
        }
        let escapes = || FsError::PermissionDenied(format!("{path} escapes the root"));
        let to_check = if follow {
            Some(full.clone())
        } else {
            full.parent().map(Path::to_path_buf)
        };
        if let Some(check) = to_check {
            match tokio::fs::canonicalize(&check).await {
                Ok(real) if !real.starts_with(&self.root) => return Err(escapes()),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(map_io(e, &check)),
            }
        }
        Ok(full)
    }
}

#[async_trait]
impl RemoteFs for RootedFs {
    fn protocol(&self) -> Protocol {
        Protocol::Sftp
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            chmod: cfg!(unix),
            set_modified: true,
            resume_upload: true,
            resume_download: true,
        }
    }

    async fn home(&self) -> Result<RemotePath, FsError> {
        Ok(RemotePath::root())
    }

    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        read_dir(&self.resolve(dir, true).await?).await
    }

    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        let real = self.resolve(path, false).await?;
        let mut entry = stat(&real).await?;
        if let (Some(e), true) = (entry.as_mut(), *path == RemotePath::root()) {
            e.name = "/".to_owned();
        }
        Ok(entry)
    }

    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        let real = self.resolve(path, false).await?;
        tokio::fs::create_dir(&real)
            .await
            .map_err(|e| map_io(e, &real))
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        let (from, to) = (
            self.resolve(from, false).await?,
            self.resolve(to, false).await?,
        );
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| map_io(e, &from))
    }

    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        let real = self.resolve(path, false).await?;
        tokio::fs::remove_file(&real)
            .await
            .map_err(|e| map_io(e, &real))
    }

    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        let real = self.resolve(path, false).await?;
        tokio::fs::remove_dir(&real)
            .await
            .map_err(|e| map_io(e, &real))
    }

    #[cfg(unix)]
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        use std::os::unix::fs::PermissionsExt;
        let real = self.resolve(path, true).await?;
        tokio::fs::set_permissions(&real, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|e| map_io(e, &real))
    }

    #[cfg(not(unix))]
    async fn chmod(&self, _: &RemotePath, _: u32) -> Result<(), FsError> {
        Err(FsError::Unsupported("chmod"))
    }

    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        let real = self.resolve(path, true).await?;
        let mut options = std::fs::OpenOptions::new();
        // Windows needs write access to set times; on unix a read-only handle also works for
        // directories.
        options.read(true).write(cfg!(windows));
        let file = tokio::fs::OpenOptions::from(options)
            .open(&real)
            .await
            .map_err(|e| map_io(e, &real))?;
        let file = file.into_std().await;
        tokio::task::spawn_blocking(move || file.set_modified(time))
            .await
            .map_err(|e| FsError::LocalIo(e.to_string()))?
            .map_err(|e| map_io(e, &real))
    }

    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let real = self.resolve(path, true).await?;
        let mut file = tokio::fs::File::open(&real)
            .await
            .map_err(|e| map_io(e, &real))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| map_io(e, &real))?;
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            let n = file.read(&mut buf).await.map_err(|e| map_io(e, &real))?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n])
                .await
                .map_err(|e| FsError::LocalIo(e.to_string()))?;
            total += n as u64;
            progress.advance(total);
        }
        sink.flush()
            .await
            .map_err(|e| FsError::LocalIo(e.to_string()))?;
        Ok(total)
    }

    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let real = self.resolve(path, true).await?;
        let mut file = if offset == 0 {
            tokio::fs::File::create(&real)
                .await
                .map_err(|e| map_io(e, &real))?
        } else {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&real)
                .await
                .map_err(|e| map_io(e, &real))?;
            let len = file.metadata().await.map_err(|e| map_io(e, &real))?.len();
            if len != offset {
                return Err(FsError::Protocol {
                    code: None,
                    message: format!("cannot resume at {offset}: remote file is {len} bytes"),
                });
            }
            file.seek(std::io::SeekFrom::End(0))
                .await
                .map_err(|e| map_io(e, &real))?;
            file
        };
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            let n = source
                .read(&mut buf)
                .await
                .map_err(|e| FsError::LocalIo(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .await
                .map_err(|e| map_io(e, &real))?;
            total += n as u64;
            progress.advance(total);
        }
        file.flush().await.map_err(|e| map_io(e, &real))?;
        Ok(total)
    }

    async fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RemotePath {
        RemotePath::parse(s).unwrap()
    }

    #[tokio::test]
    async fn read_dir_reports_kinds_sizes_and_missing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), b"hello").unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        let mut entries = read_dir(tmp.path()).await.unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (
                entries[0].name.as_str(),
                entries[0].kind.clone(),
                entries[0].size
            ),
            ("d", EntryKind::Dir, 0)
        );
        assert_eq!(
            (
                entries[1].name.as_str(),
                entries[1].kind.clone(),
                entries[1].size
            ),
            ("f.txt", EntryKind::File, 5)
        );
        assert!(matches!(
            read_dir(&tmp.path().join("nope")).await,
            Err(FsError::NotFound(_))
        ));
        assert_eq!(stat(&tmp.path().join("nope")).await.unwrap(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_are_reported_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("elsewhere", tmp.path().join("link")).unwrap();
        let e = stat(&tmp.path().join("link")).await.unwrap().unwrap();
        assert_eq!(
            e.kind,
            EntryKind::Symlink {
                target: Some("elsewhere".into())
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rooted_fs_refuses_to_escape_through_a_symlink() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"x").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("out")).unwrap();
        let fs = RootedFs::new(root.path()).unwrap();

        assert!(matches!(
            fs.list(&p("/out")).await,
            Err(FsError::PermissionDenied(_))
        ));
        assert!(matches!(
            fs.stat(&p("/out/secret")).await,
            Err(FsError::PermissionDenied(_))
        ));
        assert!(matches!(
            fs.mkdir(&p("/out/new")).await,
            Err(FsError::PermissionDenied(_))
        ));
        assert!(!outside.path().join("new").exists());
        // the link itself is visible and removable
        assert!(fs.stat(&p("/out")).await.unwrap().is_some());
        fs.remove_file(&p("/out")).await.unwrap();
        assert!(outside.path().join("secret").exists());
    }

    #[tokio::test]
    async fn resume_upload_requires_the_exact_offset() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("f"), b"abc").unwrap();
        let fs = RootedFs::new(root.path()).unwrap();
        let mut src: &[u8] = b"def";
        let err = fs
            .upload(&p("/f"), 2, &mut src, &crate::NoProgress)
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::Protocol { .. }));
        let mut src: &[u8] = b"def";
        fs.upload(&p("/f"), 3, &mut src, &crate::NoProgress)
            .await
            .unwrap();
        assert_eq!(std::fs::read(root.path().join("f")).unwrap(), b"abcdef");
    }
}
