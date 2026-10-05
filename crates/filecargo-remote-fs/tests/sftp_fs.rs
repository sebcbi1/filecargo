#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! SFTP behavior beyond the shared contract suite.

mod support;

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use filecargo_remote_fs::{EntryKind, FsError, NoProgress};
use support::{docker_sftp, sample_bytes, sha256_hex};
use tokio::io::AsyncWrite;

struct FailingSink;

impl AsyncWrite for FailingSink {
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

#[tokio::test]
async fn sftp_session_reports_protocol_and_home() {
    let (session, _, _dir) = docker_sftp().await;
    assert_eq!(session.info.protocol, filecargo_config::Protocol::Sftp);
    assert!(
        session
            .fs
            .stat(&session.info.home)
            .await
            .unwrap()
            .unwrap()
            .is_dir()
    );
    session.fs.close().await;
}

#[tokio::test]
async fn sftp_fs_a_failing_sink_is_a_local_io_error_not_a_disconnect() {
    let (session, _, _dir) = docker_sftp().await;
    let base = session.info.home.join("fc-sink-test").unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let file = base.join("f").unwrap();
    let mut src: &[u8] = b"some bytes";
    session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .unwrap();

    let err = session
        .fs
        .download(&file, 0, &mut FailingSink, &NoProgress)
        .await
        .unwrap_err();
    assert!(matches!(err, FsError::LocalIo(_)), "{err:?}");
    assert!(!err.is_retryable());
    // the connection is still usable afterwards
    assert!(session.fs.stat(&file).await.unwrap().is_some());
    session.fs.remove_all(&base).await.unwrap();
    session.fs.close().await;
}

#[tokio::test]
async fn sftp_fs_listing_reports_file_and_dir_kinds() {
    let (session, _, _dir) = docker_sftp().await;
    let base = session.info.home.join("fc-link-test").unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let target = base.join("target").unwrap();
    let mut src: &[u8] = b"x";
    session
        .fs
        .upload(&target, 0, &mut src, &NoProgress)
        .await
        .unwrap();
    session.fs.mkdir(&base.join("dir").unwrap()).await.unwrap();
    let mut kinds: Vec<_> = session
        .fs
        .list(&base)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.kind))
        .collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        kinds,
        [
            ("dir".to_owned(), EntryKind::Dir),
            ("target".to_owned(), EntryKind::File)
        ]
    );
    session.fs.remove_all(&base).await.unwrap();
    session.fs.close().await;
}

#[tokio::test]
async fn sftp_fs_throughput_of_a_20_mb_round_trip() {
    let (session, _, _dir) = docker_sftp().await;
    let base = session.info.home.join("fc-speed-test").unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let file = base.join("big").unwrap();
    let data = sample_bytes(20 * 1024 * 1024);

    let started = Instant::now();
    let mut src = &data[..];
    session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .unwrap();
    let up = started.elapsed();
    let started = Instant::now();
    let mut out = Vec::new();
    session
        .fs
        .download(&file, 0, &mut out, &NoProgress)
        .await
        .unwrap();
    let down = started.elapsed();

    let mb = data.len() as f64 / 1_048_576.0;
    eprintln!(
        "sftp throughput over loopback docker: up {:.0} MB/s, down {:.0} MB/s",
        mb / up.as_secs_f64(),
        mb / down.as_secs_f64()
    );
    assert_eq!(sha256_hex(&out), sha256_hex(&data));
    session.fs.remove_all(&base).await.unwrap();
    session.fs.close().await;
}
