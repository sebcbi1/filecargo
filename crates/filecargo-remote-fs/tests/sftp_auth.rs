#![cfg(feature = "integration")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! SSH authentication against the docker `sftp` server: `fcuser` / `fcpass`, keys in
//! `tests/docker/keys`.

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use filecargo_config::{
    Auth, ExposeSecret, MemoryStore, Paths, Protocol, SecretKey, SecretStore, SecretString, Site,
};
use filecargo_remote_fs::sftp::{authenticate, connect_ssh};
use filecargo_remote_fs::{ConnectContext, ConnectError, CredentialPrompt, TrustDecision};
use support::TestPrompter;

struct Env {
    _dir: tempfile::TempDir,
    ctx: ConnectContext,
    prompter: Arc<TestPrompter>,
    secrets: Arc<MemoryStore>,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let prompter = TestPrompter::new();
    prompter.trust(TrustDecision::TrustOnce);
    let secrets = Arc::new(MemoryStore::new());
    let mut ctx = ConnectContext::new(
        Paths::from_override(Some(dir.path().join("cfg"))),
        secrets.clone(),
        prompter.clone(),
    );
    ctx.user_known_hosts = None;
    Env {
        _dir: dir,
        ctx,
        prompter,
        secrets,
    }
}

fn site(auth: Auth) -> Site {
    let mut site = Site::new("docker-sftp", Protocol::Sftp, "127.0.0.1");
    site.port = Some(2222);
    site.user = "fcuser".to_owned();
    site.auth = auth;
    site
}

fn key_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/docker/keys")
        .join(name)
}

async fn login(site: &Site, env: &Env) -> Result<(), ConnectError> {
    let mut conn = connect_ssh(site, &env.ctx).await?;
    authenticate(&mut conn, site, &env.ctx).await
}

#[tokio::test]
async fn sftp_auth_password_is_prompted_then_reused_from_session_trust() {
    let env = env();
    let site = site(Auth::Password { remember: false });
    env.prompter.answer(&["fcpass"], false);
    login(&site, &env).await.unwrap();
    assert_eq!(env.prompter.credential_prompt_count(), 1);
    assert!(
        env.secrets.is_empty(),
        "remember=false must not touch the keychain"
    );

    login(&site, &env).await.unwrap();
    assert_eq!(
        env.prompter.credential_prompt_count(),
        1,
        "second connect must not prompt"
    );
}

#[tokio::test]
async fn sftp_auth_password_is_stored_in_the_keychain_when_the_user_says_remember() {
    let env = env();
    let site = site(Auth::Password { remember: true });
    env.prompter.answer(&["fcpass"], true);
    login(&site, &env).await.unwrap();
    let stored = env
        .secrets
        .get(&SecretKey::Password(site.id))
        .unwrap()
        .unwrap();
    assert_eq!(stored.expose_secret(), "fcpass");
}

#[tokio::test]
async fn sftp_auth_password_a_rejected_stored_password_prompts_once_with_retry() {
    let env = env();
    let site = site(Auth::Password { remember: true });
    env.secrets
        .set(
            &SecretKey::Password(site.id),
            &SecretString::from("stale".to_owned()),
        )
        .unwrap();
    env.prompter.answer(&["fcpass"], true);
    login(&site, &env).await.unwrap();

    let prompts = env.prompter.credential_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(matches!(
        &prompts[0],
        CredentialPrompt::Password { retry: true, .. }
    ));
    drop(prompts);
    let stored = env
        .secrets
        .get(&SecretKey::Password(site.id))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.expose_secret(),
        "fcpass",
        "the corrected password replaces the stale one"
    );
}

#[tokio::test]
async fn sftp_auth_password_second_failure_is_auth_failed() {
    let env = env();
    let site = site(Auth::Password { remember: false });
    env.prompter.answer(&["wrong-1"], false);
    env.prompter.answer(&["wrong-2"], false);
    let err = login(&site, &env).await.unwrap_err();
    match err {
        ConnectError::AuthFailed { methods_tried } => {
            assert!(
                methods_tried.contains(&"password".to_owned()),
                "{methods_tried:?}"
            );
        }
        other => panic!("expected AuthFailed, got {other:?}"),
    }
    let prompts = env.prompter.credential_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(matches!(
        &prompts[0],
        CredentialPrompt::Password { retry: false, .. }
    ));
    assert!(matches!(
        &prompts[1],
        CredentialPrompt::Password { retry: true, .. }
    ));
}

#[tokio::test]
async fn sftp_auth_password_cancelled_prompt_is_cancelled() {
    let env = env();
    let site = site(Auth::Password { remember: false });
    // no scripted answer: the prompter cancels
    let err = login(&site, &env).await.unwrap_err();
    assert!(matches!(err, ConnectError::Cancelled), "{err:?}");
}

#[tokio::test]
async fn sftp_auth_plain_key_needs_no_prompt() {
    let env = env();
    let site = site(Auth::KeyFile {
        path: key_path("id_plain"),
        remember_passphrase: false,
    });
    login(&site, &env).await.unwrap();
    assert_eq!(env.prompter.credential_prompt_count(), 0);
}

#[tokio::test]
async fn sftp_auth_encrypted_key_prompts_once_then_uses_session_trust() {
    let env = env();
    let site = site(Auth::KeyFile {
        path: key_path("id_encrypted"),
        remember_passphrase: false,
    });
    env.prompter.answer(&["filecargo-test-pass"], false);
    login(&site, &env).await.unwrap();
    {
        let prompts = env.prompter.credential_prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(matches!(
            &prompts[0],
            CredentialPrompt::Passphrase { retry: false, .. }
        ));
    }
    login(&site, &env).await.unwrap();
    assert_eq!(
        env.prompter.credential_prompt_count(),
        1,
        "2nd connect must not prompt"
    );
}

#[tokio::test]
async fn sftp_auth_wrong_passphrase_retries_once_then_fails() {
    let env = env();
    let site = site(Auth::KeyFile {
        path: key_path("id_encrypted"),
        remember_passphrase: false,
    });
    env.prompter.answer(&["nope"], false);
    env.prompter.answer(&["still-nope"], false);
    let err = login(&site, &env).await.unwrap_err();
    assert!(matches!(err, ConnectError::KeyFile(_)), "{err:?}");
    let prompts = env.prompter.credential_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(matches!(
        &prompts[1],
        CredentialPrompt::Passphrase { retry: true, .. }
    ));
}

#[tokio::test]
async fn sftp_auth_missing_key_file_is_a_key_file_error() {
    let env = env();
    let site = site(Auth::KeyFile {
        path: key_path("does-not-exist"),
        remember_passphrase: false,
    });
    let err = login(&site, &env).await.unwrap_err();
    assert!(matches!(err, ConnectError::KeyFile(_)), "{err:?}");
}

#[tokio::test]
async fn sftp_auth_agent_not_running_is_reported() {
    let mut env = env();
    env.ctx.agent_socket = Some(env._dir.path().join("no-such-agent.sock"));
    let site = site(Auth::Agent);
    let err = login(&site, &env).await.unwrap_err();
    match err {
        ConnectError::AuthFailed { methods_tried } => {
            assert_eq!(methods_tried, ["agent (not running)"]);
        }
        other => panic!("expected AuthFailed, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sftp_auth_agent_uses_a_loaded_identity() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let mut env = env();
    let sock = env._dir.path().join("agent.sock");
    let started = Command::new("ssh-agent")
        .arg("-a")
        .arg(&sock)
        .stdout(Stdio::null())
        .output();
    let Ok(started) = started else {
        eprintln!("ssh-agent not installed; skipping");
        return;
    };
    assert!(started.status.success());
    let pid = String::from_utf8_lossy(
        &Command::new("pgrep")
            .arg("-f")
            .arg(sock.to_str().unwrap())
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .lines()
    .next()
    .unwrap_or_default()
    .to_owned();

    // ssh-add refuses key files that are group/world readable
    let key = env._dir.path().join("id");
    std::fs::copy(key_path("id_plain"), &key).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let add = Command::new("ssh-add")
        .arg(&key)
        .env("SSH_AUTH_SOCK", &sock)
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    env.ctx.agent_socket = Some(sock);
    let result = login(&site(Auth::Agent), &env).await;
    if !pid.is_empty() {
        let _ = Command::new("kill").arg(&pid).status();
    }
    result.unwrap();
    assert_eq!(env.prompter.credential_prompt_count(), 0);
}
