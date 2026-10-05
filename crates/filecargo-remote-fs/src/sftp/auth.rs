//! SSH authentication: password (then keyboard-interactive with the same secret), key file
//! with optional passphrase, and ssh-agent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filecargo_config::{Auth, ExposeSecret, SecretKey, SecretString, Site};
use russh::MethodKind;
use russh::client::KeyboardInteractiveAuthResponse;
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{Error as KeyError, PrivateKey, PrivateKeyWithHashAlg, decode_secret_key};

use super::SshConnection;
use crate::credentials::{self, password_prompt};
use crate::{ConnectContext, ConnectError, CredentialPrompt};

const MAX_KEYBOARD_ROUNDS: usize = 5;

fn net(e: impl std::fmt::Display) -> ConnectError {
    ConnectError::Network(e.to_string())
}

fn auth_failed(methods: &[&str]) -> ConnectError {
    ConnectError::AuthFailed {
        methods_tried: methods.iter().map(|m| (*m).to_owned()).collect(),
    }
}

/// Authenticates an established, host-key-verified connection as `site.user`.
pub async fn authenticate(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
) -> Result<(), ConnectError> {
    match &site.auth {
        Auth::Password { remember } => password(conn, site, ctx, *remember).await,
        Auth::KeyFile {
            path,
            remember_passphrase,
        } => key_file(conn, site, ctx, path, *remember_passphrase).await,
        Auth::Agent => agent(conn, site, ctx).await,
        Auth::Anonymous => Err(auth_failed(&["anonymous (not valid for SFTP)"])),
    }
}

// ---- password / keyboard-interactive ---------------------------------------------------

async fn password(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
    remember: bool,
) -> Result<(), ConnectError> {
    let key = SecretKey::Password(site.id);
    let mut tried: Vec<&str> = Vec::new();
    for attempt in 0..2u8 {
        let obtained = credentials::obtain(ctx, key, remember, attempt, |retry| {
            password_prompt(site, retry)
        })
        .await?;
        if try_password(conn, site, ctx, &obtained.value, &mut tried).await? {
            credentials::succeeded(ctx, key, remember, &obtained);
            tracing::info!(target: "filecargo::protocol", site = %site.name, "SSH password accepted");
            return Ok(());
        }
        credentials::failed(ctx, &key);
    }
    Err(auth_failed(&tried))
}

async fn try_password(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
    secret: &SecretString,
    tried: &mut Vec<&'static str>,
) -> Result<bool, ConnectError> {
    if !tried.contains(&"password") {
        tried.push("password");
    }
    let result = conn
        .handle
        .authenticate_password(site.user.clone(), secret.expose_secret().to_owned())
        .await
        .map_err(net)?;
    let remaining = match result {
        russh::client::AuthResult::Success => return Ok(true),
        russh::client::AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    if !remaining.contains(&MethodKind::KeyboardInteractive) {
        return Ok(false);
    }
    if !tried.contains(&"keyboard-interactive") {
        tried.push("keyboard-interactive");
    }
    keyboard_interactive(conn, site, ctx, secret).await
}

async fn keyboard_interactive(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
    secret: &SecretString,
) -> Result<bool, ConnectError> {
    let mut response = conn
        .handle
        .authenticate_keyboard_interactive_start(site.user.clone(), None)
        .await
        .map_err(net)?;
    for round in 0..MAX_KEYBOARD_ROUNDS {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let answers = if prompts.is_empty() {
                    Vec::new()
                } else if prompts.len() == 1 && round == 0 {
                    vec![secret.expose_secret().to_owned()]
                } else {
                    let asked = CredentialPrompt::KeyboardInteractive {
                        site: site.name.clone(),
                        name,
                        instructions,
                        prompts: prompts.into_iter().map(|p| (p.prompt, p.echo)).collect(),
                    };
                    let answer = ctx
                        .prompter
                        .credential(asked)
                        .await
                        .ok_or(ConnectError::Cancelled)?;
                    answer
                        .values
                        .iter()
                        .map(|v| v.expose_secret().to_owned())
                        .collect()
                };
                response = conn
                    .handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(net)?;
            }
        }
    }
    Ok(false)
}

// ---- key file ----------------------------------------------------------------------------

fn expand_tilde(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

async fn load_key(
    site: &Site,
    ctx: &ConnectContext,
    path: &Path,
    remember_passphrase: bool,
) -> Result<PrivateKey, ConnectError> {
    let path = expand_tilde(path);
    let text = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| ConnectError::KeyFile(format!("{}: {e}", path.display())))?;
    match decode_secret_key(&text, None) {
        Ok(key) => return Ok(key),
        Err(KeyError::KeyIsEncrypted) => {}
        Err(e) => return Err(ConnectError::KeyFile(format!("{}: {e}", path.display()))),
    }

    let secret_key = SecretKey::Passphrase(site.id);
    for attempt in 0..2u8 {
        let obtained =
            credentials::obtain(ctx, secret_key, remember_passphrase, attempt, |retry| {
                CredentialPrompt::Passphrase {
                    site: site.name.clone(),
                    key_path: path.clone(),
                    retry,
                }
            })
            .await?;
        match decode_secret_key(&text, Some(obtained.value.expose_secret())) {
            Ok(key) => {
                credentials::succeeded(ctx, secret_key, remember_passphrase, &obtained);
                return Ok(key);
            }
            Err(_) => credentials::failed(ctx, &secret_key),
        }
    }
    Err(ConnectError::KeyFile(format!(
        "{}: wrong passphrase",
        path.display()
    )))
}

async fn key_file(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
    path: &Path,
    remember_passphrase: bool,
) -> Result<(), ConnectError> {
    let key = load_key(site, ctx, path, remember_passphrase).await?;
    let hash = conn
        .handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let result = conn
        .handle
        .authenticate_publickey(
            site.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
        )
        .await
        .map_err(net)?;
    if result.success() {
        tracing::info!(target: "filecargo::protocol", site = %site.name, "SSH key accepted");
        Ok(())
    } else {
        Err(auth_failed(&["publickey"]))
    }
}

// ---- ssh-agent ---------------------------------------------------------------------------

type DynAgent = AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>>;

#[cfg(unix)]
async fn connect_agent(ctx: &ConnectContext) -> Result<DynAgent, KeyError> {
    Ok(match &ctx.agent_socket {
        Some(path) => AgentClient::connect_uds(path).await?.dynamic(),
        None => AgentClient::connect_env().await?.dynamic(),
    })
}

#[cfg(windows)]
async fn connect_agent(ctx: &ConnectContext) -> Result<DynAgent, KeyError> {
    let pipe = ctx
        .agent_socket
        .clone()
        .unwrap_or_else(|| PathBuf::from(r"\\.\pipe\openssh-ssh-agent"));
    match AgentClient::connect_named_pipe(&pipe).await {
        Ok(agent) => Ok(agent.dynamic()),
        Err(_) => Ok(AgentClient::connect_pageant().await?.dynamic()),
    }
}

async fn agent(
    conn: &mut SshConnection,
    site: &Site,
    ctx: &ConnectContext,
) -> Result<(), ConnectError> {
    let mut agent = connect_agent(ctx)
        .await
        .map_err(|_| auth_failed(&["agent (not running)"]))?;
    let identities = agent.request_identities().await.map_err(net)?;
    let hash = conn
        .handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        let result = conn
            .handle
            .authenticate_publickey_with(site.user.clone(), key, hash, &mut agent)
            .await
            .map_err(net)?;
        if result.success() {
            tracing::info!(target: "filecargo::protocol", site = %site.name, "SSH agent identity accepted");
            return Ok(());
        }
    }
    Err(auth_failed(&["agent"]))
}
