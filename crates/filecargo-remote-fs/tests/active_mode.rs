#![cfg(all(feature = "integration", target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Active-mode FTP (the server connects back to us). Published localhost ports hide the client
//! behind NAT, so this talks to the container's own IP on the docker bridge: Linux only. The
//! `ftp-active` server refuses PASV / EPSV, so a transfer that works there was active.

mod support;

use std::process::Command;
use std::sync::Arc;

use filecargo_config::{Auth, FtpMode, MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::{ConnectContext, FsError, NoProgress, Session, connect};
use support::{TestPrompter, sample_bytes, sha256_hex};

fn container_ip(container: &str) -> Option<String> {
    let out = Command::new("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            container,
        ])
        .output()
        .ok()?;
    let ip = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !ip.is_empty()).then_some(ip)
}

async fn dial(mode: FtpMode) -> Session {
    let Some(ip) = container_ip("filecargo-test-ftp-active-1") else {
        panic!(
            "the docker test servers are not running \
             (docker compose -f tests/docker/compose.yml up -d --wait)"
        );
    };
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    prompter.answer(&["ftppass"], false);
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
        prompter,
    );
    ctx.user_known_hosts = None;
    let mut site = Site::new("docker-ftp-active", Protocol::Ftp, ip);
    site.port = Some(2124);
    site.user = "ftpuser".to_owned();
    site.auth = Auth::Password { remember: false };
    site.ftp_mode = mode;
    connect(&site, &ctx).await.unwrap()
}

#[tokio::test]
async fn ftp_passive_mode_is_refused_by_the_active_only_server() {
    // the control experiment: without it the next test could be passing in passive mode
    let session = dial(FtpMode::Passive).await;
    let err = session.fs.list(&session.info.home).await.unwrap_err();
    assert!(!matches!(err, FsError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn ftp_active_mode_lists_and_transfers() {
    let session = dial(FtpMode::Active).await;
    let base = session
        .info
        .home
        .join(&format!("fc-active-{}", std::process::id()))
        .unwrap();
    session.fs.mkdir(&base).await.unwrap();
    assert!(
        session
            .fs
            .list(&session.info.home)
            .await
            .unwrap()
            .iter()
            .any(|e| e.is_dir())
    );

    let file = base.join("data.bin").unwrap();
    let data = sample_bytes(1024 * 1024);
    let mut src = &data[..];
    session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .unwrap();
    let mut out = Vec::new();
    session
        .fs
        .download(&file, 0, &mut out, &NoProgress)
        .await
        .unwrap();
    assert_eq!(sha256_hex(&out), sha256_hex(&data));
    session.fs.remove_all(&base).await.unwrap();
    session.fs.close().await;
}
