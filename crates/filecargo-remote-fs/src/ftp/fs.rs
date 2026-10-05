use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use filecargo_config::Protocol;
use suppaftp::tokio::AsyncRustlsFtpStream;
use suppaftp::{FtpError, FtpResult, Status};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, MutexGuard};

use super::time::format_timestamp;
use super::{list, mlsd};
use crate::{Capabilities, Entry, EntryKind, FsError, Progress, RemoteFs, RemotePath};

/// What `FEAT` said, upper-cased.
#[derive(Debug, Default, Clone)]
pub(crate) struct Features(HashSet<String>);

impl Features {
    pub(crate) fn from_feat(feat: &suppaftp::types::Features) -> Self {
        Self(feat.keys().map(|k| k.to_ascii_uppercase()).collect())
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

pub(crate) struct Inner {
    pub(crate) ftp: AsyncRustlsFtpStream,
    /// Set while a call is in flight and left set when it was cancelled or the connection is no
    /// longer trustworthy: a half-finished command or transfer leaves the control connection in
    /// an unknown state.
    broken: bool,
}

/// FTP / FTPS filesystem. One control connection, one command or transfer at a time.
pub struct FtpFs {
    inner: Mutex<Inner>,
    pub(crate) features: Features,
    protocol: Protocol,
    pub(crate) timeout: Duration,
}

/// Exclusive use of the control connection for one call. If the future holding it is dropped
/// before [`Op::finish`], the connection stays marked broken.
pub(crate) struct Op<'a> {
    guard: MutexGuard<'a, Inner>,
}

impl Op<'_> {
    pub(crate) fn ftp(&mut self) -> &mut AsyncRustlsFtpStream {
        &mut self.guard.ftp
    }

    pub(crate) fn finish<T>(mut self, result: Result<T, FsError>) -> Result<T, FsError> {
        let connection_lost = matches!(
            result,
            Err(FsError::Disconnected(_) | FsError::Timeout | FsError::TlsSessionReuseRequired)
        );
        self.guard.broken = connection_lost;
        result
    }
}

pub(crate) fn map_ftp(error: &FtpError, what: &str) -> FsError {
    match error {
        FtpError::ConnectionError(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            FsError::Timeout
        }
        FtpError::ConnectionError(io) => FsError::Disconnected(io.to_string()),
        FtpError::SecureError(message) => FsError::Disconnected(format!("TLS: {message}")),
        FtpError::UnexpectedResponse(response) => {
            let text = String::from_utf8_lossy(&response.body).into_owned();
            let lowered = text.to_ascii_lowercase();
            match response.status {
                Status::NotAvailable => FsError::Disconnected(text),
                Status::CannotOpenDataConnection | Status::TransferAborted => {
                    FsError::Disconnected(format!("data connection: {text}"))
                }
                Status::FileUnavailable
                | Status::BadFilename
                | Status::RequestFileActionIgnored => {
                    if lowered.contains("permission denied") || lowered.contains("access denied") {
                        FsError::PermissionDenied(what.to_owned())
                    } else if lowered.contains("not empty") {
                        FsError::DirectoryNotEmpty(what.to_owned())
                    } else if lowered.contains("exists") {
                        FsError::AlreadyExists(what.to_owned())
                    } else if lowered.contains("not a directory") {
                        FsError::NotADirectory(what.to_owned())
                    } else {
                        FsError::NotFound(what.to_owned())
                    }
                }
                status => FsError::Protocol {
                    code: u16::try_from(status.code()).ok().filter(|c| *c != 0),
                    message: format!("{what}: {text}"),
                },
            }
        }
        other => FsError::Protocol {
            code: None,
            message: format!("{what}: {other}"),
        },
    }
}

/// Runs one library call under the per-command timeout.
pub(crate) async fn timed<T>(
    limit: Duration,
    what: &str,
    call: impl Future<Output = FtpResult<T>>,
) -> Result<T, FsError> {
    match tokio::time::timeout(limit, call).await {
        Ok(result) => result.map_err(|e| map_ftp(&e, what)),
        Err(_) => Err(FsError::Timeout),
    }
}

/// A path as a control-connection argument. CR / LF would end the command line and start
/// another one.
fn arg(path: &RemotePath) -> Result<&str, FsError> {
    if path.as_str().contains(['\r', '\n']) {
        return Err(FsError::Protocol {
            code: None,
            message: "line breaks are not allowed in FTP paths".to_owned(),
        });
    }
    Ok(path.as_str())
}

impl FtpFs {
    pub(crate) fn new(
        ftp: AsyncRustlsFtpStream,
        features: Features,
        protocol: Protocol,
        timeout: Duration,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner { ftp, broken: false }),
            features,
            protocol,
            timeout,
        }
    }

    /// Takes the control connection, or fails if an earlier call left it in an unknown state.
    pub(crate) async fn begin(&self) -> Result<Op<'_>, FsError> {
        let mut guard = self.inner.lock().await;
        if guard.broken {
            return Err(FsError::Disconnected(
                "the connection was interrupted and cannot be reused".to_owned(),
            ));
        }
        guard.broken = true;
        Ok(Op { guard })
    }

    async fn list_raw(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        let path = arg(dir)?;
        let mut op = self.begin().await?;
        let result = if self.features.has("MLSD") {
            timed(self.timeout, path, op.ftp().mlsd(Some(path)))
                .await
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(|l| match mlsd::parse_line(l) {
                            mlsd::Parsed::Entry(e) => Some(e),
                            mlsd::Parsed::Skip => None,
                        })
                        .collect()
                })
        } else {
            timed(
                self.timeout,
                path,
                op.ftp().list(Some(&format!("-a {path}"))),
            )
            .await
            .map(|lines| lines.iter().filter_map(|l| list::parse_line(l)).collect())
        };
        op.finish(result)
    }
}

#[async_trait]
impl RemoteFs for FtpFs {
    fn protocol(&self) -> Protocol {
        self.protocol
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            chmod: true,
            set_modified: self.features.has("MFMT"),
            resume_upload: true,
            resume_download: true,
        }
    }

    async fn home(&self) -> Result<RemotePath, FsError> {
        let mut op = self.begin().await?;
        let result = timed(self.timeout, "PWD", op.ftp().pwd()).await;
        let result = result.and_then(|p| {
            RemotePath::parse(&p).map_err(|e| FsError::Protocol {
                code: None,
                message: e.to_string(),
            })
        });
        op.finish(result)
    }

    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        self.list_raw(dir).await
    }

    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        let Some(parent) = path.parent() else {
            let mut root = Entry::new("/", EntryKind::Dir);
            root.permissions = None;
            return Ok(Some(root));
        };
        let wanted = path.file_name().unwrap_or_default();
        let argument = arg(path)?;
        if self.features.has("MLST") {
            let mut op = self.begin().await?;
            let reply = timed(self.timeout, argument, op.ftp().mlst(Some(argument))).await;
            return match op.finish(reply) {
                Ok(line) => Ok(match mlsd::parse_line(line.trim_start_matches(' ')) {
                    mlsd::Parsed::Entry(mut e) => {
                        // MLST names the entry by its full path on some servers
                        e.name = wanted.to_owned();
                        Some(e)
                    }
                    mlsd::Parsed::Skip => None,
                }),
                Err(FsError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            };
        }
        match self.list_raw(&parent).await {
            Ok(entries) => Ok(entries.into_iter().find(|e| e.name == wanted)),
            Err(FsError::NotFound(_) | FsError::NotADirectory(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        let argument = arg(path)?;
        let mut op = self.begin().await?;
        let result = timed(self.timeout, argument, op.ftp().mkdir(argument)).await;
        let failed = result.is_err();
        let result = op.finish(result);
        match result {
            Err(FsError::NotFound(_)) if failed && matches!(self.stat(path).await, Ok(Some(_))) => {
                Err(FsError::AlreadyExists(path.to_string()))
            }
            other => other,
        }
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        let (from_arg, to_arg) = (arg(from)?, arg(to)?);
        let mut op = self.begin().await?;
        let result = timed(self.timeout, from_arg, op.ftp().rename(from_arg, to_arg)).await;
        op.finish(result)
    }

    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        let argument = arg(path)?;
        let mut op = self.begin().await?;
        let result = timed(self.timeout, argument, op.ftp().rm(argument)).await;
        op.finish(result)
    }

    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        let argument = arg(path)?;
        let mut op = self.begin().await?;
        let result = timed(self.timeout, argument, op.ftp().rmdir(argument)).await;
        let result = op.finish(result);
        match result {
            Err(FsError::NotFound(_))
                if self
                    .list_raw(path)
                    .await
                    .is_ok_and(|entries| !entries.is_empty()) =>
            {
                Err(FsError::DirectoryNotEmpty(path.to_string()))
            }
            other => other,
        }
    }

    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        let argument = arg(path)?;
        let mut op = self.begin().await?;
        let command = format!("CHMOD {:o} {argument}", mode & 0o7777);
        let result = timed(self.timeout, argument, op.ftp().site(command)).await;
        let result = match result {
            Err(FsError::Protocol {
                code: Some(500 | 501 | 502 | 504),
                ..
            }) => Err(FsError::Unsupported("chmod")),
            other => other.map(|_| ()),
        };
        op.finish(result)
    }

    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        if !self.features.has("MFMT") {
            return Err(FsError::Unsupported("set_modified"));
        }
        let argument = arg(path)?;
        let stamp =
            format_timestamp(time).ok_or(FsError::Unsupported("modification times before 1970"))?;
        let mut op = self.begin().await?;
        let command = format!("MFMT {stamp} {argument}");
        let result = timed(
            self.timeout,
            argument,
            op.ftp().custom_command(command, &[Status::File]),
        )
        .await
        .map(|_| ());
        op.finish(result)
    }

    async fn download(
        &self,
        _path: &RemotePath,
        _offset: u64,
        _sink: &mut (dyn AsyncWrite + Send + Unpin),
        _progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        Err(FsError::Unsupported("download"))
    }

    async fn upload(
        &self,
        _path: &RemotePath,
        _offset: u64,
        _source: &mut (dyn AsyncRead + Send + Unpin),
        _progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        Err(FsError::Unsupported("upload"))
    }

    async fn close(&self) {
        if let Ok(mut op) = self.begin().await {
            let _ = tokio::time::timeout(self.timeout, op.ftp().quit()).await;
            // never reusable after QUIT
            op.guard.broken = true;
        }
    }
}
