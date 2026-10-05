#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FTPS: certificate prompts and pinning, implicit mode, and the TLS-session-reuse server.

mod support;

use std::sync::Arc;

use filecargo_config::{Auth, MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::{
    CertificateProblem, ConnectContext, ConnectError, FsError, NoProgress, Session, SessionTrust,
    TrustDecision, connect,
};
use support::{TestPrompter, sample_bytes, sha256_hex};

struct Env {
    dir: tempfile::TempDir,
    ctx: ConnectContext,
    prompter: Arc<TestPrompter>,
}

fn env(decision: TrustDecision) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    prompter.trust_cert(decision);
    prompter.answer(&["ftppass"], false);
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().join("cfg"))),
        Arc::new(MemoryStore::new()),
        prompter.clone(),
    );
    ctx.user_known_hosts = None;
    Env { dir, ctx, prompter }
}

fn site(protocol: Protocol, port: u16) -> Site {
    let mut site = Site::new("docker-ftps", protocol, "127.0.0.1");
    site.port = Some(port);
    site.user = "ftpuser".to_owned();
    site.auth = Auth::Password { remember: false };
    site
}

const EXPLICIT: (Protocol, u16) = (Protocol::FtpsExplicit, 2121);
const IMPLICIT: (Protocol, u16) = (Protocol::FtpsImplicit, 9990);

async fn dial(env: &Env, (protocol, port): (Protocol, u16)) -> Result<Session, ConnectError> {
    connect(&site(protocol, port), &env.ctx).await
}

#[tokio::test]
async fn ftps_self_signed_certificate_prompts_and_trust_once_is_remembered_in_memory() {
    for target in [EXPLICIT, IMPLICIT] {
        let env = env(TrustDecision::TrustOnce);
        env.prompter.answer(&["ftppass"], false); // second connect, if it asks again
        let session = dial(&env, target).await.unwrap();
        assert!(
            session
                .info
                .tls
                .as_deref()
                .is_some_and(|t| t.contains("TLS")),
            "{:?}",
            session.info.tls
        );
        assert!(session.fs.stat(&session.info.home).await.unwrap().is_some());
        session.fs.close().await;

        let prompts = env.prompter.certificate_prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 1, "{target:?}");
        assert_eq!(prompts[0].problem, CertificateProblem::SelfSigned);
        assert_eq!(prompts[0].sha256.len(), 64);
        assert!(
            prompts[0].subject.contains("localhost"),
            "{}",
            prompts[0].subject
        );

        // the same run does not ask again, and nothing is written to disk
        dial(&env, target).await.unwrap();
        assert_eq!(env.prompter.certificate_prompt_count(), 1, "{target:?}");
        assert!(!env.ctx.paths.trusted_certs().exists());
    }
}

#[tokio::test]
async fn ftps_trust_always_pins_the_certificate_across_runs() {
    let env = env(TrustDecision::TrustAlways);
    dial(&env, EXPLICIT).await.unwrap();
    assert_eq!(env.prompter.certificate_prompt_count(), 1);
    let pins = std::fs::read_to_string(env.ctx.paths.trusted_certs()).unwrap();
    assert!(
        pins.contains("host = \"127.0.0.1\"") && pins.contains("port = 2121"),
        "{pins}"
    );
    let sha = env.prompter.certificate_prompts.lock().unwrap()[0]
        .sha256
        .clone();
    assert!(pins.contains(&sha));

    // a fresh run: new in-memory trust, same files
    let mut again = env.ctx.clone();
    again.trust = Arc::new(SessionTrust::new());
    env.prompter.answer(&["ftppass"], false);
    connect(&site(EXPLICIT.0, EXPLICIT.1), &again)
        .await
        .unwrap();
    assert_eq!(
        env.prompter.certificate_prompt_count(),
        1,
        "pinned: no second prompt"
    );
}

#[tokio::test]
async fn ftps_reject_fails_with_certificate_rejected_and_pins_nothing() {
    let env = env(TrustDecision::Reject);
    let err = dial(&env, EXPLICIT).await.err().unwrap();
    assert!(matches!(err, ConnectError::CertificateRejected), "{err:?}");
    assert!(!env.ctx.paths.trusted_certs().exists());
    // and it is asked again next time: rejection is not remembered
    let err = dial(&env, EXPLICIT).await.err().unwrap();
    assert!(matches!(err, ConnectError::CertificateRejected), "{err:?}");
    assert_eq!(env.prompter.certificate_prompt_count(), 2);
    assert_eq!(
        env.prompter.credential_prompt_count(),
        0,
        "no password is sent after a rejection"
    );
}

#[tokio::test]
async fn ftps_a_different_certificate_at_the_same_host_and_port_prompts_again() {
    let env = env(TrustDecision::TrustAlways);
    // A pin for this host:port whose fingerprint is not the server's: what the file looks like
    // after the server's certificate was replaced.
    std::fs::create_dir_all(env.ctx.paths.trusted_certs().parent().unwrap()).unwrap();
    let stale = "00".repeat(32);
    std::fs::write(
        env.ctx.paths.trusted_certs(),
        format!(
            "version = 1\n\n[[cert]]\nhost = \"127.0.0.1\"\nport = 2121\nsha256 = \"{stale}\"\n"
        ),
    )
    .unwrap();
    dial(&env, EXPLICIT).await.unwrap();
    assert_eq!(
        env.prompter.certificate_prompt_count(),
        1,
        "an unknown cert must prompt"
    );
    let pins = std::fs::read_to_string(env.ctx.paths.trusted_certs()).unwrap();
    assert!(pins.contains(&stale), "the old pin is kept: {pins}");
    assert_eq!(pins.matches("sha256").count(), 2, "{pins}");
}

#[tokio::test]
async fn ftps_a_corrupt_pin_file_is_reported_not_overwritten() {
    let env = env(TrustDecision::TrustAlways);
    std::fs::create_dir_all(env.ctx.paths.trusted_certs().parent().unwrap()).unwrap();
    std::fs::write(env.ctx.paths.trusted_certs(), "garbage = [").unwrap();
    let err = dial(&env, EXPLICIT).await.err().unwrap();
    assert!(
        matches!(err, ConnectError::Fs(FsError::LocalIo(_))),
        "{err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(env.ctx.paths.trusted_certs()).unwrap(),
        "garbage = ["
    );
}

#[tokio::test]
async fn ftps_transfers_are_encrypted_end_to_end_and_hash_correctly() {
    for target in [EXPLICIT, IMPLICIT] {
        let env = env(TrustDecision::TrustOnce);
        let session = dial(&env, target).await.unwrap();
        let base = session
            .info
            .home
            .join(&format!("fc-ftps-{}-{}", target.1, std::process::id()))
            .unwrap();
        session.fs.mkdir(&base).await.unwrap();
        let file = base.join("data.bin").unwrap();
        let data = sample_bytes(3 * 1024 * 1024);
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
        assert_eq!(sha256_hex(&out), sha256_hex(&data), "{target:?}");
        session.fs.remove_all(&base).await.unwrap();
        session.fs.close().await;
        drop(env.dir);
    }
}

/// AC6: against a server that requires TLS session reuse on data connections, a data
/// transfer either succeeds (the shared rustls session cache resumed the session) or fails
/// with `TlsSessionReuseRequired`: never with a raw TLS or I/O error.
#[tokio::test]
async fn ftps_reuse_required_server_succeeds_or_reports_the_reuse_error() {
    let env = env(TrustDecision::TrustOnce);
    let session = dial(&env, (Protocol::FtpsExplicit, 2122)).await.unwrap();
    let mut outcomes = Vec::new();

    let listing = session.fs.list(&session.info.home).await;
    outcomes.push(("list", listing.map(|_| ())));

    let base = session
        .info
        .home
        .join(&format!("fc-reuse-{}", std::process::id()))
        .unwrap();
    let file = base.join("f").unwrap();
    // mkdir needs no data connection; the transfer after it does
    let made = session.fs.mkdir(&base).await;
    outcomes.push(("mkdir", made));
    let data = sample_bytes(64 * 1024);
    let mut src = &data[..];
    let up = session
        .fs
        .upload(&file, 0, &mut src, &NoProgress)
        .await
        .map(|_| ());
    outcomes.push(("upload", up));

    let mut recorded = Vec::new();
    for (what, outcome) in &outcomes {
        match outcome {
            Ok(()) => recorded.push(format!("{what}: ok")),
            Err(FsError::TlsSessionReuseRequired) => {
                recorded.push(format!("{what}: TlsSessionReuseRequired"));
            }
            Err(other) => {
                panic!("{what} failed with something other than the reuse error: {other:?}")
            }
        }
    }
    eprintln!(
        "AC6 outcome on the reuse-required server: {}",
        recorded.join(", ")
    );
}

#[tokio::test]
async fn ftps_without_session_resumption_the_reuse_server_reports_the_clear_error() {
    let mut env = env(TrustDecision::TrustOnce);
    env.ctx.tls_session_resumption = false;
    let session = dial(&env, (Protocol::FtpsExplicit, 2122)).await.unwrap();
    let err = session.fs.list(&session.info.home).await.unwrap_err();
    assert_eq!(err, FsError::TlsSessionReuseRequired, "{err:?}");
    let text = err.to_string();
    assert!(
        text.contains("TLS session reuse") && text.contains("require_ssl_reuse=NO"),
        "{text}"
    );
    assert!(!err.is_retryable());
    // the connection is not reusable afterwards
    // (the root `stat` is synthesised without touching the connection, so use a real command)
    let next = session
        .fs
        .mkdir(&session.info.home.join("after-error").unwrap())
        .await;
    assert!(matches!(next, Err(FsError::Disconnected(_))), "{next:?}");
}

#[tokio::test]
async fn ftps_without_session_resumption_a_server_that_does_not_need_it_still_works() {
    let mut env = env(TrustDecision::TrustOnce);
    env.ctx.tls_session_resumption = false;
    let session = dial(&env, EXPLICIT).await.unwrap();
    session.fs.list(&session.info.home).await.unwrap();
    session.fs.close().await;
}
