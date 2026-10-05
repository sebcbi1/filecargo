//! SSH host-key policy: the user's OpenSSH `known_hosts` is read-only, filecargo only ever
//! appends to its own file, and a changed key is never offered for acceptance.

use std::path::Path;

use russh::keys::{Error as KeyError, HashAlg, PublicKey, known_hosts};

use crate::{ConnectContext, ConnectError, FsError, HostKeyPrompt, TrustDecision};

enum Lookup {
    Match,
    Unknown,
    Changed,
}

fn lookup(path: &Path, host: &str, port: u16, key: &PublicKey) -> Lookup {
    match known_hosts::check_known_hosts_path(host, port, key, path) {
        Ok(true) => Lookup::Match,
        Ok(false) => Lookup::Unknown,
        Err(KeyError::KeyChanged { .. }) => Lookup::Changed,
        Err(error) => {
            // An unreadable or exotic line (marker, unsupported key type) only costs a prompt.
            tracing::warn!(target: "filecargo::protocol", path = %path.display(), %error, "ignoring unreadable known_hosts");
            Lookup::Unknown
        }
    }
}

pub(crate) async fn verify(
    ctx: &ConnectContext,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), ConnectError> {
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    if ctx.trust.host_key_trusted(host, port, &fingerprint) {
        return Ok(());
    }

    let own = ctx.paths.known_hosts();
    let files: Vec<&Path> = ctx
        .user_known_hosts
        .as_deref()
        .into_iter()
        .chain(std::iter::once(own.as_path()))
        .collect();
    let mut matched = false;
    for path in files {
        match lookup(path, host, port, key) {
            Lookup::Changed => {
                tracing::warn!(target: "filecargo::protocol", host, port, "SSH host key changed");
                return Err(ConnectError::HostKeyChanged {
                    host: format!("{host}:{port}"),
                    fingerprint,
                    known_in: path.to_path_buf(),
                });
            }
            Lookup::Match => matched = true,
            Lookup::Unknown => {}
        }
    }
    if matched {
        return Ok(());
    }

    let decision = ctx
        .prompter
        .host_key(HostKeyPrompt {
            host: host.to_owned(),
            port,
            algorithm: key.algorithm().to_string(),
            fingerprint: fingerprint.clone(),
        })
        .await;
    match decision {
        TrustDecision::Reject => Err(ConnectError::HostKeyRejected),
        TrustDecision::TrustOnce => {
            ctx.trust.trust_host_key(host, port, &fingerprint);
            Ok(())
        }
        TrustDecision::TrustAlways => {
            known_hosts::learn_known_hosts_path(host, port, key, &own).map_err(|e| {
                ConnectError::Fs(FsError::LocalIo(format!("{}: {e}", own.display())))
            })?;
            tracing::info!(target: "filecargo::protocol", host, port, %fingerprint, "SSH host key trusted permanently");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use filecargo_config::{MemoryStore, Paths};
    use russh::keys::parse_public_key_base64;

    use super::*;
    use crate::{CredentialAnswer, CredentialPrompt, Prompter};

    const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G2sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    struct Scripted {
        decision: TrustDecision,
        asked: Mutex<Vec<HostKeyPrompt>>,
    }

    #[async_trait]
    impl Prompter for Scripted {
        async fn credential(&self, _: CredentialPrompt) -> Option<CredentialAnswer> {
            None
        }
        async fn host_key(&self, request: HostKeyPrompt) -> TrustDecision {
            self.asked.lock().unwrap().push(request);
            self.decision
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        ctx: ConnectContext,
        prompter: std::sync::Arc<Scripted>,
    }

    fn fixture(decision: TrustDecision) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let prompter = std::sync::Arc::new(Scripted {
            decision,
            asked: Mutex::default(),
        });
        let mut ctx = ConnectContext::new(
            Paths::from_override(Some(dir.path().join("cfg"))),
            std::sync::Arc::new(MemoryStore::new()),
            prompter.clone(),
        );
        ctx.user_known_hosts = Some(dir.path().join("user_known_hosts"));
        Fixture { dir, ctx, prompter }
    }

    fn key(b64: &str) -> PublicKey {
        parse_public_key_base64(b64).unwrap()
    }

    #[tokio::test]
    async fn unknown_key_prompts_with_a_sha256_fingerprint() {
        let fx = fixture(TrustDecision::TrustOnce);
        verify(&fx.ctx, "example.org", 22, &key(KEY_A))
            .await
            .unwrap();
        let asked = fx.prompter.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(asked[0].algorithm, "ssh-ed25519");
    }

    #[tokio::test]
    async fn trust_once_is_remembered_in_memory_only() {
        let fx = fixture(TrustDecision::TrustOnce);
        verify(&fx.ctx, "h", 22, &key(KEY_A)).await.unwrap();
        verify(&fx.ctx, "h", 22, &key(KEY_A)).await.unwrap();
        assert_eq!(fx.prompter.asked.lock().unwrap().len(), 1);
        assert!(!fx.ctx.paths.known_hosts().exists());
    }

    #[tokio::test]
    async fn trust_always_writes_only_the_own_file() {
        let fx = fixture(TrustDecision::TrustAlways);
        std::fs::write(fx.dir.path().join("user_known_hosts"), "# mine\n").unwrap();
        verify(&fx.ctx, "h", 2222, &key(KEY_A)).await.unwrap();
        let own = std::fs::read_to_string(fx.ctx.paths.known_hosts()).unwrap();
        // russh writes a leading newline into a fresh file; harmless
        assert!(
            own.trim_start().starts_with("[h]:2222 ssh-ed25519 "),
            "{own}"
        );
        assert_eq!(
            std::fs::read_to_string(fx.dir.path().join("user_known_hosts")).unwrap(),
            "# mine\n"
        );
        // a fresh context (no in-memory trust) must not prompt again
        let mut again = fx.ctx.clone();
        again.trust = std::sync::Arc::new(crate::SessionTrust::new());
        verify(&again, "h", 2222, &key(KEY_A)).await.unwrap();
        assert_eq!(fx.prompter.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reject_fails_and_remembers_nothing() {
        let fx = fixture(TrustDecision::Reject);
        let err = verify(&fx.ctx, "h", 22, &key(KEY_A)).await.unwrap_err();
        assert!(matches!(err, ConnectError::HostKeyRejected));
        assert!(!fx.ctx.paths.known_hosts().exists());
        assert!(matches!(
            verify(&fx.ctx, "h", 22, &key(KEY_A)).await,
            Err(ConnectError::HostKeyRejected)
        ));
        assert_eq!(fx.prompter.asked.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn changed_key_fails_without_prompting_and_names_the_file() {
        let fx = fixture(TrustDecision::TrustAlways);
        let user_file = fx.dir.path().join("user_known_hosts");
        std::fs::write(&user_file, format!("h ssh-ed25519 {KEY_B}\n")).unwrap();
        let err = verify(&fx.ctx, "h", 22, &key(KEY_A)).await.unwrap_err();
        match err {
            ConnectError::HostKeyChanged {
                known_in,
                fingerprint,
                ..
            } => {
                assert_eq!(known_in, user_file);
                assert!(fingerprint.starts_with("SHA256:"));
            }
            other => panic!("expected HostKeyChanged, got {other:?}"),
        }
        assert!(fx.prompter.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn key_known_in_the_user_file_connects_silently() {
        let fx = fixture(TrustDecision::Reject);
        std::fs::write(
            fx.dir.path().join("user_known_hosts"),
            format!("h ssh-ed25519 {KEY_A}\n"),
        )
        .unwrap();
        verify(&fx.ctx, "h", 22, &key(KEY_A)).await.unwrap();
        assert!(fx.prompter.asked.lock().unwrap().is_empty());
    }
}
