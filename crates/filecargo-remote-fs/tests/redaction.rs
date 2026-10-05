#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! AC10: a full run of every protocol and auth method, with logging at TRACE and every `log`
//! record from the dependencies bridged into `tracing` (the worst case), leaves no password,
//! passphrase or `PASS <secret>` in the captured output. This is the only test in its binary:
//! the subscriber is process-global.

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use filecargo_config::{Auth, MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::{ConnectContext, NoProgress, Session, TrustDecision, connect};
use support::{TestPrompter, sample_bytes};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The real secrets of the docker servers, plus sentinels typed as rejected attempts.
const SECRETS: [&str; 5] = [
    "fcpass",
    "ftppass",
    "filecargo-test-pass",
    "S3NTINEL-wrong-password",
    "S3NTINEL-wrong-passphrase",
];

fn key_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/docker/keys")
        .join(name)
}

fn context(answers: &[&[&str]], decision: TrustDecision) -> ConnectContext {
    let dir = tempfile::tempdir().unwrap().keep();
    let prompter = TestPrompter::new();
    prompter.trust(decision);
    prompter.trust_cert(decision);
    for answer in answers {
        prompter.answer(answer, false);
    }
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir)),
        Arc::new(MemoryStore::new()),
        prompter,
    );
    ctx.user_known_hosts = None;
    ctx
}

fn site(protocol: Protocol, port: u16, user: &str, auth: Auth) -> Site {
    let mut site = Site::new("redaction", protocol, "127.0.0.1");
    site.port = Some(port);
    site.user = user.to_owned();
    site.auth = auth;
    site
}

async fn exercise(session: Session) {
    let base = session
        .info
        .home
        .join(&format!("fc-redact-{}", std::process::id()))
        .unwrap();
    session.fs.mkdir(&base).await.unwrap();
    let file = base.join("f").unwrap();
    let data = sample_bytes(64 * 1024);
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
    let _ = session.fs.list(&base).await.unwrap();
    session.fs.remove_all(&base).await.unwrap();
    session.fs.close().await;
}

#[tokio::test]
async fn nothing_secret_reaches_the_logs_in_a_full_run() {
    let capture = Capture::default();
    tracing_log::LogTracer::init().unwrap();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(capture.clone())
            .finish(),
    )
    .unwrap();

    let password = Auth::Password { remember: false };
    // SFTP password: a wrong attempt, then the right one
    let ctx = context(
        &[&["S3NTINEL-wrong-password"], &["fcpass"]],
        TrustDecision::TrustOnce,
    );
    exercise(
        connect(
            &site(Protocol::Sftp, 2222, "fcuser", password.clone()),
            &ctx,
        )
        .await
        .unwrap(),
    )
    .await;
    // SFTP encrypted key: a wrong passphrase, then the right one
    let ctx = context(
        &[&["S3NTINEL-wrong-passphrase"], &["filecargo-test-pass"]],
        TrustDecision::TrustOnce,
    );
    let key = Auth::KeyFile {
        path: key_path("id_encrypted"),
        remember_passphrase: false,
    };
    exercise(
        connect(&site(Protocol::Sftp, 2222, "fcuser", key), &ctx)
            .await
            .unwrap(),
    )
    .await;
    // FTP, explicit FTPS, implicit FTPS: wrong password first
    for (protocol, port) in [
        (Protocol::Ftp, 2121),
        (Protocol::FtpsExplicit, 2121),
        (Protocol::FtpsImplicit, 9990),
    ] {
        let ctx = context(
            &[&["S3NTINEL-wrong-password"], &["ftppass"]],
            TrustDecision::TrustOnce,
        );
        exercise(
            connect(&site(protocol, port, "ftpuser", password.clone()), &ctx)
                .await
                .unwrap(),
        )
        .await;
    }
    // a failed login: two wrong passwords
    let ctx = context(
        &[&["S3NTINEL-wrong-password"], &["S3NTINEL-wrong-password"]],
        TrustDecision::TrustOnce,
    );
    assert!(
        connect(&site(Protocol::Ftp, 2121, "ftpuser", password), &ctx)
            .await
            .is_err()
    );

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("filecargo::protocol"),
        "the run must have logged something"
    );
    assert!(
        log.contains("> PASS ***"),
        "FTP logins are logged, redacted"
    );
    for secret in SECRETS {
        assert!(
            !log.contains(secret),
            "the log contains {secret:?}:\n{}",
            excerpt(&log, secret)
        );
    }
    for line in log.lines().filter(|l| l.contains("PASS ")) {
        assert!(line.contains("PASS ***"), "unredacted PASS line: {line}");
    }
    eprintln!(
        "checked {} log lines for {} secrets",
        log.lines().count(),
        SECRETS.len()
    );
}

fn excerpt(log: &str, needle: &str) -> String {
    log.lines()
        .filter(|l| l.contains(needle))
        .take(5)
        .collect::<Vec<_>>()
        .join("\n")
}
