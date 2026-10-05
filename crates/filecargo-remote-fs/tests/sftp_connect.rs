#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! SSH transport and host-key policy against the docker `sftp` server (port 2222).

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{MemoryStore, Paths, Protocol, Site};
use filecargo_remote_fs::sftp::connect_ssh;
use filecargo_remote_fs::{ConnectContext, ConnectError, TrustDecision};
use support::TestPrompter;

struct Env {
    dir: tempfile::TempDir,
    ctx: ConnectContext,
    prompter: Arc<TestPrompter>,
}

impl Env {
    fn new(decision: TrustDecision) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let prompter = TestPrompter::new();
        prompter.trust(decision);
        let mut ctx = ConnectContext::new(
            Paths::from_override(Some(dir.path().join("cfg"))),
            Arc::new(MemoryStore::new()),
            prompter.clone(),
        );
        ctx.user_known_hosts = Some(dir.path().join("ssh_known_hosts"));
        Self { dir, ctx, prompter }
    }

    fn own_known_hosts(&self) -> PathBuf {
        self.ctx.paths.known_hosts()
    }

    fn user_known_hosts(&self) -> PathBuf {
        self.dir.path().join("ssh_known_hosts")
    }
}

fn site() -> Site {
    let mut site = Site::new("docker-sftp", Protocol::Sftp, "127.0.0.1");
    site.port = Some(2222);
    site.user = "fcuser".to_owned();
    site
}

#[tokio::test]
async fn sftp_host_keys_unknown_key_prompts_and_trust_always_persists() {
    let env = Env::new(TrustDecision::TrustAlways);
    std::fs::write(env.user_known_hosts(), "# the user's own file\n").unwrap();

    let conn = connect_ssh(&site(), &env.ctx).await.unwrap();
    assert!(!conn.is_closed());
    assert_eq!(env.prompter.host_key_prompt_count(), 1);
    let prompt = env.prompter.host_key_prompts.lock().unwrap()[0].clone();
    assert!(prompt.fingerprint.starts_with("SHA256:"));
    assert_eq!((prompt.host.as_str(), prompt.port), ("127.0.0.1", 2222));

    let own = std::fs::read_to_string(env.own_known_hosts()).unwrap();
    assert!(own.trim_start().starts_with("[127.0.0.1]:2222 "), "{own}");
    assert_eq!(
        std::fs::read_to_string(env.user_known_hosts()).unwrap(),
        "# the user's own file\n",
        "the user's known_hosts must stay byte-identical"
    );

    // a brand-new context sharing only the files must not prompt
    let mut again = env.ctx.clone();
    again.trust = Arc::new(filecargo_remote_fs::SessionTrust::new());
    connect_ssh(&site(), &again).await.unwrap();
    assert_eq!(env.prompter.host_key_prompt_count(), 1);
}

#[tokio::test]
async fn sftp_host_keys_reject_fails_and_writes_nothing() {
    let env = Env::new(TrustDecision::Reject);
    let err = connect_ssh(&site(), &env.ctx).await.unwrap_err();
    assert!(matches!(err, ConnectError::HostKeyRejected), "{err:?}");
    assert!(!env.own_known_hosts().exists());
    assert!(!env.user_known_hosts().exists());
}

#[tokio::test]
async fn sftp_host_keys_trust_once_does_not_prompt_twice_in_a_run() {
    let env = Env::new(TrustDecision::TrustOnce);
    connect_ssh(&site(), &env.ctx).await.unwrap();
    connect_ssh(&site(), &env.ctx).await.unwrap();
    assert_eq!(env.prompter.host_key_prompt_count(), 1);
    assert!(!env.own_known_hosts().exists());
}

#[tokio::test]
async fn sftp_host_keys_changed_key_is_refused_without_prompting() {
    let env = Env::new(TrustDecision::TrustAlways);
    // A different ed25519 key recorded for this host:port.
    let other = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    let line = format!("[127.0.0.1]:2222 ssh-ed25519 {other}\n");
    std::fs::write(env.user_known_hosts(), &line).unwrap();

    let err = connect_ssh(&site(), &env.ctx).await.unwrap_err();
    match err {
        ConnectError::HostKeyChanged { known_in, .. } => {
            assert_eq!(known_in, env.user_known_hosts())
        }
        other => panic!("expected HostKeyChanged, got {other:?}"),
    }
    assert_eq!(env.prompter.host_key_prompt_count(), 0);
    assert_eq!(
        std::fs::read_to_string(env.user_known_hosts()).unwrap(),
        line
    );
}

#[tokio::test]
async fn sftp_host_keys_refused_port_is_reported_as_refused() {
    let env = Env::new(TrustDecision::TrustOnce);
    let mut site = site();
    site.port = Some(1);
    let err = connect_ssh(&site, &env.ctx).await.unwrap_err();
    assert!(matches!(err, ConnectError::Refused(_)), "{err:?}");
}
