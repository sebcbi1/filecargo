#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! AC7: a real shell on the docker SFTP server, drawn by the emulator.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use filecargo_config::{Auth, MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::{
    CertificatePrompt, ConnectContext, CredentialAnswer, CredentialPrompt, HostKeyPrompt, Prompter,
    TrustDecision, connect,
};
use filecargo_terminal::{TermSize, TerminalHandle, spawn};

struct Auto;

#[async_trait]
impl Prompter for Auto {
    async fn credential(&self, _: CredentialPrompt) -> Option<CredentialAnswer> {
        Some(CredentialAnswer {
            values: vec![secrecy::SecretString::from("fcpass".to_owned())],
            remember: false,
        })
    }
    async fn host_key(&self, _: HostKeyPrompt) -> TrustDecision {
        TrustDecision::TrustOnce
    }
    async fn certificate(&self, _: CertificatePrompt) -> TrustDecision {
        TrustDecision::Reject
    }
}

async fn wait_for_row(term: &TerminalHandle, wanted: &str) {
    for _ in 0..400 {
        let found = term.with_screen(|s| s.contents().lines().any(|l| l.trim_end() == wanted));
        if found {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let screen = term.with_screen(|s| s.contents());
    panic!("no row equal to {wanted:?} appeared; the screen shows:\n{screen}");
}

#[tokio::test]
async fn a_real_shell_prints_output_and_follows_a_resize() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().to_path_buf())),
        Arc::new(MemoryStore::new()),
        Arc::new(Auto),
    );
    ctx.user_known_hosts = None;
    let mut site = Site::new("docker-sftp", Protocol::Sftp, "127.0.0.1");
    site.port = Some(2222);
    site.user = "fcuser".to_owned();
    site.auth = Auth::Password { remember: false };
    let session = connect(&site, &ctx).await.unwrap();

    let channel = session
        .shell
        .as_ref()
        .expect("SFTP sessions have a shell")
        .open("xterm-256color", 80, 24)
        .await
        .unwrap();
    let term = spawn(channel, TermSize { cols: 80, rows: 24 }, 5000);

    term.send_text("printf 'ok\\n'\n");
    // the echo of the command is on one row; the output `ok` alone on another
    wait_for_row(&term, "ok").await;

    term.resize(TermSize {
        cols: 100,
        rows: 30,
    });
    tokio::time::sleep(Duration::from_millis(300)).await; // let SIGWINCH land
    term.send_text("stty size\n");
    wait_for_row(&term, "30 100").await;
    assert_eq!(term.with_screen(|s| s.size()), (30, 100));

    term.close();
    session.fs.close().await;
}
