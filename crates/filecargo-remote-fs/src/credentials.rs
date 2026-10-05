//! Where a password or key passphrase comes from, shared by the SFTP and FTP backends:
//! session memory, then the keychain (only if the site asks to remember), then the prompter.

use filecargo_config::{SecretKey, SecretString, Site};

use crate::{ConnectContext, ConnectError, CredentialAnswer, CredentialPrompt};

/// A credential about to be tried, and whether the user just typed it.
pub(crate) struct Obtained {
    pub value: SecretString,
    prompted: bool,
    remember: bool,
}

/// Attempt 0 uses a stored credential when there is one; attempt 1 (after a rejection) always
/// asks, with `retry: true`. `prompt(retry)` builds the question.
pub(crate) async fn obtain(
    ctx: &ConnectContext,
    key: SecretKey,
    keychain_allowed: bool,
    attempt: u8,
    prompt: impl FnOnce(bool) -> CredentialPrompt,
) -> Result<Obtained, ConnectError> {
    if attempt == 0 {
        if let Some(value) = ctx.trust.credential(&key) {
            return Ok(Obtained {
                value,
                prompted: false,
                remember: false,
            });
        }
        if keychain_allowed {
            match ctx.secrets.get(&key) {
                Ok(Some(value)) => {
                    return Ok(Obtained {
                        value,
                        prompted: false,
                        remember: false,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(target: "filecargo::protocol", %error, "keychain lookup failed; asking instead");
                }
            }
        }
    }
    let answer: CredentialAnswer = ctx
        .prompter
        .credential(prompt(attempt > 0))
        .await
        .ok_or(ConnectError::Cancelled)?;
    let value = answer
        .values
        .into_iter()
        .next()
        .ok_or(ConnectError::Cancelled)?;
    Ok(Obtained {
        value,
        prompted: true,
        remember: answer.remember,
    })
}

/// The credential worked: keep a typed one for this run, and in the keychain if the site and
/// the user both allow it.
pub(crate) fn succeeded(
    ctx: &ConnectContext,
    key: SecretKey,
    keychain_allowed: bool,
    obtained: &Obtained,
) {
    if !obtained.prompted {
        return;
    }
    ctx.trust.remember_credential(key, &obtained.value);
    if keychain_allowed
        && obtained.remember
        && let Err(error) = ctx.secrets.set(&key, &obtained.value)
    {
        tracing::warn!(target: "filecargo::protocol", %error, "could not store the secret in the keychain");
    }
}

/// The server rejected it: forget the in-memory copy so it is not offered again.
pub(crate) fn failed(ctx: &ConnectContext, key: &SecretKey) {
    ctx.trust.forget_credential(key);
}

pub(crate) fn password_prompt(site: &Site, retry: bool) -> CredentialPrompt {
    CredentialPrompt::Password {
        site: site.name.clone(),
        user: site.user.clone(),
        retry,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use filecargo_config::{ExposeSecret, MemoryStore, Paths, SecretStore, SiteId};

    use super::*;
    use crate::{HostKeyPrompt, Prompter, TrustDecision};

    #[derive(Default)]
    struct Typed {
        asked: Mutex<Vec<bool>>,
    }

    #[async_trait]
    impl Prompter for Typed {
        async fn credential(&self, request: CredentialPrompt) -> Option<CredentialAnswer> {
            let CredentialPrompt::Password { retry, .. } = request else {
                return None;
            };
            self.asked.lock().unwrap().push(retry);
            Some(CredentialAnswer {
                values: vec![SecretString::from("typed".to_owned())],
                remember: true,
            })
        }
        async fn host_key(&self, _: HostKeyPrompt) -> TrustDecision {
            TrustDecision::Reject
        }
    }

    fn setup() -> (ConnectContext, Arc<Typed>, Arc<MemoryStore>, Site) {
        let prompter = Arc::new(Typed::default());
        let secrets = Arc::new(MemoryStore::new());
        let ctx = ConnectContext::new(
            Paths::from_override(Some(std::env::temp_dir().join("fc-unused"))),
            secrets.clone(),
            prompter.clone(),
        );
        let mut site = Site::new("s", filecargo_config::Protocol::Sftp, "h");
        site.id = SiteId::new();
        (ctx, prompter, secrets, site)
    }

    #[tokio::test]
    async fn lookup_order_is_session_then_keychain_then_prompt() {
        let (ctx, prompter, secrets, site) = setup();
        let key = SecretKey::Password(site.id);
        let ask = |retry| password_prompt(&site, retry);

        // nothing stored: prompt
        let got = obtain(&ctx, key, true, 0, ask).await.unwrap();
        assert_eq!(got.value.expose_secret(), "typed");
        assert_eq!(prompter.asked.lock().unwrap().len(), 1);

        // keychain beats the prompt, but only when allowed
        secrets
            .set(&key, &SecretString::from("from-keychain".to_owned()))
            .unwrap();
        let got = obtain(&ctx, key, true, 0, ask).await.unwrap();
        assert_eq!(got.value.expose_secret(), "from-keychain");
        let got = obtain(&ctx, key, false, 0, ask).await.unwrap();
        assert_eq!(got.value.expose_secret(), "typed");

        // session memory beats the keychain
        ctx.trust
            .remember_credential(key, &SecretString::from("from-session".to_owned()));
        let got = obtain(&ctx, key, true, 0, ask).await.unwrap();
        assert_eq!(got.value.expose_secret(), "from-session");
    }

    #[tokio::test]
    async fn the_retry_attempt_always_prompts_with_retry_set() {
        let (ctx, prompter, secrets, site) = setup();
        let key = SecretKey::Password(site.id);
        secrets
            .set(&key, &SecretString::from("stale".to_owned()))
            .unwrap();
        let got = obtain(&ctx, key, true, 1, |retry| password_prompt(&site, retry))
            .await
            .unwrap();
        assert_eq!(got.value.expose_secret(), "typed");
        assert_eq!(*prompter.asked.lock().unwrap(), [true]);
    }

    #[tokio::test]
    async fn success_remembers_a_typed_secret_in_session_and_optionally_the_keychain() {
        let (ctx, _, secrets, site) = setup();
        let key = SecretKey::Password(site.id);
        let typed = obtain(&ctx, key, true, 0, |r| password_prompt(&site, r))
            .await
            .unwrap();
        succeeded(&ctx, key, false, &typed);
        assert!(ctx.trust.credential(&key).is_some());
        assert!(secrets.get(&key).unwrap().is_none(), "keychain not allowed");
        succeeded(&ctx, key, true, &typed);
        assert_eq!(secrets.get(&key).unwrap().unwrap().expose_secret(), "typed");
        failed(&ctx, &key);
        assert!(ctx.trust.credential(&key).is_none());
    }

    #[tokio::test]
    async fn a_cancelled_prompt_is_reported_as_cancelled() {
        struct Cancel;
        #[async_trait]
        impl Prompter for Cancel {
            async fn credential(&self, _: CredentialPrompt) -> Option<CredentialAnswer> {
                None
            }
            async fn host_key(&self, _: HostKeyPrompt) -> TrustDecision {
                TrustDecision::Reject
            }
        }
        let (mut ctx, _, _, site) = setup();
        ctx.prompter = Arc::new(Cancel);
        let key = SecretKey::Password(site.id);
        let result = obtain(&ctx, key, false, 0, |r| password_prompt(&site, r)).await;
        assert!(matches!(result, Err(ConnectError::Cancelled)));
    }
}
