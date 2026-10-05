#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Fault injection: a `RemoteFs` wrapper that drops the connection (or hangs) after N bytes of
//! a transfer, and that, like the FTP backend, is unusable once a transfer was cancelled.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::SystemTime;

use async_trait::async_trait;
use filecargo_config::Protocol;
use filecargo_remote_fs::{Capabilities, Entry, FsError, Progress, RemoteFs, RemotePath};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Move this many bytes, then fail with `Disconnected` (the partial data stays).
    FailAfter(u64),
    /// Move this many bytes, then never finish.
    HangAfter(u64),
}

/// Shared by every connection the connector hands out.
#[derive(Default)]
pub struct FlakyControl {
    faults: Mutex<VecDeque<Fault>>,
    /// `(direction, offset)` of every transfer call, in order.
    pub calls: Mutex<Vec<(&'static str, u64)>>,
    /// The next this-many `connect`s fail with a connect error.
    pub fail_connects: AtomicUsize,
}

impl FlakyControl {
    /// The next transfer call gets this fault (queued after earlier ones).
    pub fn inject(&self, fault: Fault) {
        self.faults.lock().unwrap().push_back(fault);
    }

    /// Consumes one pending connect failure, if any.
    pub fn take_connect_failure(&self) -> bool {
        let mut current = self.fail_connects.load(Ordering::SeqCst);
        while current > 0 {
            match self.fail_connects.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
        false
    }

    fn next_fault(&self) -> Option<Fault> {
        self.faults.lock().unwrap().pop_front()
    }

    pub fn calls(&self) -> Vec<(&'static str, u64)> {
        self.calls.lock().unwrap().clone()
    }
}

pub struct FlakyFs {
    inner: Arc<dyn RemoteFs>,
    control: Arc<FlakyControl>,
    broken: Arc<AtomicBool>,
}

impl FlakyFs {
    pub fn new(inner: Arc<dyn RemoteFs>, control: Arc<FlakyControl>) -> Self {
        Self {
            inner,
            control,
            broken: Arc::new(AtomicBool::new(false)),
        }
    }

    fn check(&self) -> Result<(), FsError> {
        if self.broken.load(Ordering::SeqCst) {
            Err(FsError::Disconnected(
                "the connection was interrupted".to_owned(),
            ))
        } else {
            Ok(())
        }
    }
}

/// Marks the connection broken unless the transfer ran to the end: dropping the future of a
/// transfer that was cancelled leaves it that way, as on a real FTP control connection.
struct BrokenUnlessDone {
    broken: Arc<AtomicBool>,
    done: bool,
}

impl Drop for BrokenUnlessDone {
    fn drop(&mut self) {
        if !self.done {
            self.broken.store(true, Ordering::SeqCst);
        }
    }
}

/// Accepts `left` bytes, then fails every write.
struct LimitedSink<'a> {
    inner: &'a mut (dyn AsyncWrite + Send + Unpin),
    left: u64,
    tripped: bool,
}

impl AsyncWrite for LimitedSink<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.left == 0 {
            self.tripped = true;
            return Poll::Ready(Err(std::io::Error::other("injected fault")));
        }
        let take = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let result = Pin::new(&mut *self.inner).poll_write(cx, &buf[..take]);
        if let Poll::Ready(Ok(n)) = &result {
            self.left -= *n as u64;
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[async_trait]
impl RemoteFs for FlakyFs {
    fn protocol(&self) -> Protocol {
        self.inner.protocol()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn home(&self) -> Result<RemotePath, FsError> {
        self.check()?;
        self.inner.home().await
    }
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError> {
        self.check()?;
        self.inner.list(dir).await
    }
    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError> {
        self.check()?;
        self.inner.stat(path).await
    }
    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.mkdir(path).await
    }
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.rename(from, to).await
    }
    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.remove_file(path).await
    }
    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError> {
        self.check()?;
        self.inner.remove_dir(path).await
    }
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError> {
        self.check()?;
        self.inner.chmod(path, mode).await
    }
    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError> {
        self.check()?;
        self.inner.set_modified(path, time).await
    }

    async fn download(
        &self,
        path: &RemotePath,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        self.check()?;
        self.control
            .calls
            .lock()
            .unwrap()
            .push(("download", offset));
        let mut guard = BrokenUnlessDone {
            broken: self.broken.clone(),
            done: false,
        };
        let fault = self.control.next_fault();
        let result = match fault {
            None => self.inner.download(path, offset, sink, progress).await,
            Some(Fault::FailAfter(n) | Fault::HangAfter(n)) => {
                let mut limited = LimitedSink {
                    inner: sink,
                    left: n,
                    tripped: false,
                };
                let result = self
                    .inner
                    .download(path, offset, &mut limited, progress)
                    .await;
                if limited.tripped {
                    if matches!(fault, Some(Fault::HangAfter(_))) {
                        std::future::pending::<()>().await;
                    }
                    Err(FsError::Disconnected("connection reset by peer".to_owned()))
                } else {
                    result
                }
            }
        };
        guard.done = !matches!(result, Err(FsError::Disconnected(_)));
        result
    }

    async fn upload(
        &self,
        path: &RemotePath,
        offset: u64,
        source: &mut (dyn AsyncRead + Send + Unpin),
        progress: &dyn Progress,
    ) -> Result<u64, FsError> {
        self.check()?;
        self.control.calls.lock().unwrap().push(("upload", offset));
        let mut guard = BrokenUnlessDone {
            broken: self.broken.clone(),
            done: false,
        };
        let fault = self.control.next_fault();
        let result = match fault {
            None => self.inner.upload(path, offset, source, progress).await,
            Some(Fault::FailAfter(n) | Fault::HangAfter(n)) => {
                let mut limited = (&mut *source).take(n);
                let sent = self
                    .inner
                    .upload(path, offset, &mut limited, progress)
                    .await?;
                if sent >= n {
                    if matches!(fault, Some(Fault::HangAfter(_))) {
                        std::future::pending::<()>().await;
                    }
                    Err(FsError::Disconnected("connection reset by peer".to_owned()))
                } else {
                    Ok(sent)
                }
            }
        };
        guard.done = !matches!(result, Err(FsError::Disconnected(_)));
        result
    }

    async fn close(&self) {
        self.inner.close().await;
    }
}
