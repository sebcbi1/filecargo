#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Cancellation (dropping a future), keepalive, and the secrecy of FTP logs.

mod support;

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use filecargo_remote_fs::{FsError, NoProgress};
use support::{docker_ftp, docker_ftp_with_keepalive, sample_bytes};
use tokio::io::AsyncWrite;

/// Accepts `limit` bytes, then never completes a write: a download into it hangs mid-transfer
/// until its future is dropped.
struct StallingSink {
    seen: usize,
    limit: usize,
}

impl AsyncWrite for StallingSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.seen >= self.limit {
            return Poll::Pending;
        }
        self.seen += buf.len();
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn ftp_cancel_dropping_a_download_breaks_the_connection_and_a_fresh_connect_works() {
    let (session, _, _dir) = docker_ftp(2121).await;
    let base = session
        .info
        .home
        .join(&format!("fc-cancel-{}", std::process::id()))
        .unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let file = base.join("big").unwrap();
    let data = sample_bytes(8 * 1024 * 1024);
    let mut src = &data[..];
    session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .unwrap();

    let mut sink = StallingSink {
        seen: 0,
        limit: 1024 * 1024,
    };
    let cancelled = tokio::time::timeout(
        Duration::from_millis(500),
        session.fs.download(&file, 0, &mut sink, &NoProgress),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the download was supposed to stall and be cancelled"
    );
    assert!(
        sink.seen >= 1024 * 1024,
        "the cancel must hit mid-transfer, saw {}",
        sink.seen
    );

    // Every later call on the broken connection is `Disconnected`, and retryable.
    for result in [
        session.fs.list(&base).await.map(|_| ()),
        session.fs.stat(&file).await.map(|_| ()),
        session.fs.mkdir(&base.join("x").unwrap()).await,
    ] {
        let err = result.unwrap_err();
        assert!(matches!(err, FsError::Disconnected(_)), "{err:?}");
        assert!(err.is_retryable());
    }

    // A new connection is fine, and the partial transfer left the file intact.
    let (fresh, _, _dir2) = docker_ftp(2121).await;
    assert_eq!(
        fresh.fs.stat(&file).await.unwrap().unwrap().size,
        data.len() as u64
    );
    fresh.fs.remove_all(&base).await.unwrap();
}

#[tokio::test]
async fn ftp_cancel_a_failing_sink_leaves_the_connection_usable() {
    struct Failing;
    impl AsyncWrite for Failing {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Err(std::io::Error::other("disk full")))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    let (session, _, _dir) = docker_ftp(2121).await;
    let base = session
        .info
        .home
        .join(&format!("fc-sink-{}", std::process::id()))
        .unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let file = base.join("f").unwrap();
    let data = sample_bytes(2 * 1024 * 1024);
    let mut src = &data[..];
    session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .unwrap();

    let err = session
        .fs
        .download(&file, 0, &mut Failing, &NoProgress)
        .await
        .unwrap_err();
    assert!(matches!(err, FsError::LocalIo(_)), "{err:?}");
    assert!(!err.is_retryable());
    // the deferred reply of the aborted transfer is consumed by the next command
    assert!(session.fs.stat(&file).await.unwrap().is_some());
    let mut out = Vec::new();
    session
        .fs
        .download(&file, 0, &mut out, &NoProgress)
        .await
        .unwrap();
    assert_eq!(out.len(), data.len());
    session.fs.remove_all(&base).await.unwrap();
}

#[tokio::test]
async fn ftp_keepalive_keeps_an_idle_connection_alive() {
    // port 2123's server drops control connections idle for 5 s
    let session = docker_ftp_with_keepalive(2123, 1).await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    session.fs.home().await.unwrap();
}

#[tokio::test]
async fn ftp_without_keepalive_the_idle_connection_is_dropped() {
    // the control experiment: shows the previous test would fail without the NOOPs
    let session = docker_ftp_with_keepalive(2123, 3600).await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let err = session.fs.home().await.unwrap_err();
    assert!(err.is_retryable(), "{err:?}");
}
