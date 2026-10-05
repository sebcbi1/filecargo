use std::io::SeekFrom;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use filecargo_config::Protocol;
use russh::Disconnect;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::Metadata;
use russh_sftp::client::{Config as SftpConfig, SftpSession};
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

use super::SshConnection;
use crate::{
    Capabilities, ConnectContext, ConnectError, Entry, EntryKind, FsError, Progress, RemoteFs,
    RemotePath,
};

const CHUNK: usize = 256 * 1024;

/// SFTP over an authenticated SSH connection.
pub struct SftpFs {
    sftp: SftpSession,
    conn: Arc<SshConnection>,
}

pub(crate) async fn open_sftp(
    conn: Arc<SshConnection>,
    ctx: &ConnectContext,
) -> Result<SftpFs, ConnectError> {
    let net = |e: &dyn std::fmt::Display| ConnectError::Network(e.to_string());
    let channel = conn
        .handle
        .channel_open_session()
        .await
        .map_err(|e| net(&e))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| net(&e))?;
    let config = SftpConfig {
        request_timeout_secs: u64::from(ctx.timeouts.timeout_secs).max(10),
        ..SftpConfig::default()
    };
    let sftp = SftpSession::new_with_config(channel.into_stream(), config)
        .await
        .map_err(|e| ConnectError::Fs(map_err(&e, "sftp subsystem")))?;
    Ok(SftpFs { sftp, conn })
}

fn map_err(error: &SftpError, what: &str) -> FsError {
    match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => FsError::NotFound(what.to_owned()),
            StatusCode::PermissionDenied => FsError::PermissionDenied(what.to_owned()),
            StatusCode::NoConnection | StatusCode::ConnectionLost => {
                FsError::Disconnected(error.to_string())
            }
            StatusCode::OpUnsupported => FsError::Unsupported("operation"),
            _ => FsError::Protocol {
                code: None,
                message: format!("{what}: {error}"),
            },
        },
        SftpError::Timeout => FsError::Timeout,
        SftpError::IO(_) => FsError::Disconnected(error.to_string()),
        _ => FsError::Protocol {
            code: None,
            message: format!("{what}: {error}"),
        },
    }
}

/// Errors surfaced through `AsyncRead` / `AsyncWrite` on an SFTP file arrive as `io::Error`
/// wrapping the original [`SftpError`].
fn map_io(error: &std::io::Error, what: &str) -> FsError {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SftpError>())
    {
        Some(inner) => map_err(inner, what),
        None => FsError::Disconnected(format!("{what}: {error}")),
    }
}

fn to_entry(name: String, attrs: &Metadata, symlink_target: Option<String>) -> Entry {
    let kind = match attrs.file_type() {
        FileType::Dir => EntryKind::Dir,
        FileType::File => EntryKind::File,
        FileType::Symlink => EntryKind::Symlink {
            target: symlink_target,
        },
        FileType::Other => EntryKind::Other,
    };
    Entry {
        size: if kind == EntryKind::Dir {
            0
        } else {
            attrs.size.unwrap_or(0)
        },
        kind,
        modified: attrs
            .mtime
            .map(|s| UNIX_EPOCH + Duration::from_secs(u64::from(s))),
        permissions: attrs.permissions.map(|p| p & 0o7777),
        owner: None,
        group: None,
        name,
    }
}

impl SftpFs {
    async fn symlink_target(&self, path: &str) -> Option<String> {
        self.sftp.read_link(path).await.ok()
    }
}

#[async_trait]
impl RemoteFs for SftpFs {
    fn protocol(&self) -> Protocol {
        Protocol::Sftp
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            chmod: true,
            set_modified: true,
            resume_upload: true,
            resume_download: true,
        }
    }

    async fn home(&self) -> Result<RemotePath, FsError> {
        let home = self
            .sftp
            .canonicalize(".")
            .await
            .map_err(|e| map_err(&e, "."))?;
        RemotePath::parse(&home).map_err(|e| FsError::Protocol {
            code: None,
            message: e.to_string(),
        })
    }

    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        let listing = self
            .sftp
            .read_dir(dir.as_str())
            .await
            .map_err(|e| map_err(&e, dir.as_str()))?;
        let mut entries = Vec::new();
        for item in listing {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = item.metadata();
            let target = if attrs.file_type() == FileType::Symlink {
                self.symlink_target(&item.path()).await
            } else {
                None
            };
            entries.push(to_entry(name, &attrs, target));
        }
        Ok(entries)
    }

    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        match self.sftp.symlink_metadata(path.as_str()).await {
            Ok(attrs) => {
                let target = if attrs.file_type() == FileType::Symlink {
                    self.symlink_target(path.as_str()).await
                } else {
                    None
                };
                Ok(Some(to_entry(
                    path.file_name().unwrap_or("/").to_owned(),
                    &attrs,
                    target,
                )))
            }
            Err(SftpError::Status(s)) if s.status_code == StatusCode::NoSuchFile => Ok(None),
            Err(e) => Err(map_err(&e, path.as_str())),
        }
    }

    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        match self.sftp.create_dir(path.as_str()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // OpenSSH answers "failure" for an existing directory.
                if matches!(self.stat(path).await, Ok(Some(_))) {
                    Err(FsError::AlreadyExists(path.to_string()))
                } else {
                    Err(map_err(&e, path.as_str()))
                }
            }
        }
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        self.sftp
            .rename(from.as_str(), to.as_str())
            .await
            .map_err(|e| map_err(&e, from.as_str()))
    }

    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        self.sftp
            .remove_file(path.as_str())
            .await
            .map_err(|e| map_err(&e, path.as_str()))
    }

    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        match self.sftp.remove_dir(path.as_str()).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if self
                    .list(path)
                    .await
                    .is_ok_and(|entries| !entries.is_empty())
                {
                    Err(FsError::DirectoryNotEmpty(path.to_string()))
                } else {
                    Err(map_err(&e, path.as_str()))
                }
            }
        }
    }

    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        // Only the fields we mean to change: `set_metadata` sends every `Some` field.
        let attrs = FileAttributes {
            permissions: Some(mode & 0o7777),
            ..FileAttributes::empty()
        };
        self.sftp
            .set_metadata(path.as_str(), attrs)
            .await
            .map_err(|e| map_err(&e, path.as_str()))
    }

    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        let secs = time
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| u32::try_from(d.as_secs()).ok())
            .ok_or(FsError::Unsupported(
                "modification times outside 1970..2106",
            ))?;
        let current = self
            .sftp
            .metadata(path.as_str())
            .await
            .map_err(|e| map_err(&e, path.as_str()))?;
        // SFTPv3 sets access and modification time together.
        let attrs = FileAttributes {
            atime: Some(current.atime.unwrap_or(secs)),
            mtime: Some(secs),
            ..FileAttributes::empty()
        };
        self.sftp
            .set_metadata(path.as_str(), attrs)
            .await
            .map_err(|e| map_err(&e, path.as_str()))
    }

    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let mut file = self
            .sftp
            .open(path.as_str())
            .await
            .map_err(|e| map_err(&e, path.as_str()))?;
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))
                .await
                .map_err(|e| map_io(&e, path.as_str()))?;
        }
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            let n = file
                .read(&mut buf)
                .await
                .map_err(|e| map_io(&e, path.as_str()))?;
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
        // The read-only handle holds no pending writes; a failed close tells us nothing new.
        let _ = file.close().await;
        Ok(total)
    }

    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        let mut file = if offset == 0 {
            self.sftp
                .open_with_flags(
                    path.as_str(),
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                )
                .await
        } else {
            let size = self
                .sftp
                .metadata(path.as_str())
                .await
                .map_err(|e| map_err(&e, path.as_str()))?
                .size
                .unwrap_or(0);
            if size != offset {
                return Err(FsError::Protocol {
                    code: None,
                    message: format!("cannot resume at {offset}: remote file is {size} bytes"),
                });
            }
            self.sftp
                .open_with_flags(path.as_str(), OpenFlags::WRITE)
                .await
        }
        .map_err(|e| map_err(&e, path.as_str()))?;
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))
                .await
                .map_err(|e| map_io(&e, path.as_str()))?;
        }

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
                .map_err(|e| map_io(&e, path.as_str()))?;
            total += n as u64;
            progress.advance(total);
        }
        // Closing awaits every pending write, so write errors surface here, not on drop.
        file.close().await.map_err(|e| map_io(&e, path.as_str()))?;
        Ok(total)
    }

    async fn close(&self) {
        let _ = self.sftp.close().await;
        let _ = self
            .conn
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}
