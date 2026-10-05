//! How sessions are opened.

use std::sync::Arc;

use async_trait::async_trait;
use filecargo_config::{Site, SiteId};
use filecargo_remote_fs::{
    ConnectContext, ConnectError, Entry, FsError, RemoteFs, RemotePath, Session, SessionInfo,
};

use crate::app::{Core, Msg};
use crate::command::Command;
use crate::sort::{retain_visible, sort_entries};
use crate::state::{ConnectStep, Level, Pane, PromptKind, SessionState, TerminalState};

/// The browsing session and every transfer worker connection go through this. The default
/// calls `remote_fs::connect`; tests serve a directory on disk.
#[async_trait]
pub trait SessionFactory: Send + Sync {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError>;
}

pub(crate) struct RealFactory;

#[async_trait]
impl SessionFactory for RealFactory {
    async fn connect(&self, site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError> {
        filecargo_remote_fs::connect(site, ctx).await
    }
}

pub(crate) fn default_factory() -> Arc<dyn SessionFactory> {
    Arc::new(RealFactory)
}

/// The connected session the actor holds.
pub(crate) struct Live {
    pub site: SiteId,
    pub fs: Arc<dyn RemoteFs>,
    pub info: SessionInfo,
}

/// What the connect task found.
pub(crate) struct Connected {
    pub epoch: u64,
    pub site: SiteId,
    pub result: Result<Opened, ConnectError>,
}

pub(crate) struct Opened {
    pub session: filecargo_remote_fs::Session,
    pub start: RemotePath,
    pub listing: Result<Vec<Entry>, FsError>,
}

pub(crate) struct RemoteListing {
    pub seq: u64,
    pub result: Result<(RemotePath, Vec<Entry>), FsError>,
}

/// Errors that mean the user must read something (credentials, trust), as opposed to a network
/// hiccup that the log and the `Failed` state already say.
fn needs_a_message(error: &ConnectError) -> bool {
    matches!(
        error,
        ConnectError::AuthFailed { .. }
            | ConnectError::KeyFile(_)
            | ConnectError::HostKeyRejected
            | ConnectError::HostKeyChanged { .. }
            | ConnectError::CertificateRejected
            | ConnectError::Tls(_)
    )
}

impl Core {
    /// Opens `site`, replacing any session (one at a time in v1). Transfers keep running on
    /// their own connections. `then` runs once connected (an automatic reconnect resumes the
    /// command that triggered it).
    pub(crate) fn connect(&mut self, site_id: SiteId, then: Option<Command>) {
        let Some(site) = self.state.servers.site(site_id).cloned() else {
            self.message(
                Level::Error,
                "Cannot connect",
                "That site no longer exists.",
            );
            return;
        };
        self.drop_session();
        self.connect_epoch += 1;
        let epoch = self.connect_epoch;
        self.after_connect = then;
        self.state.session = SessionState::Connecting {
            site: site_id,
            step: ConnectStep::Connecting,
        };
        self.state.remote = None;
        self.changed();
        tracing::info!(target: "filecargo::app", site = %site.name, "connecting");

        let mut ctx = self.ctx.clone();
        ctx.timeouts = self.state.settings.connection.clone();
        let factory = self.factory.clone();
        let messages = self.messages.clone();
        tokio::spawn(async move {
            let result = async {
                let session = factory.connect(&site, &ctx).await?;
                let _ = messages.send(Msg::ConnectStep {
                    epoch,
                    step: ConnectStep::Listing,
                });
                let wanted = site
                    .remote_dir
                    .as_deref()
                    .and_then(|d| RemotePath::parse(d).ok());
                let start = match wanted {
                    Some(dir)
                        if session
                            .fs
                            .stat(&dir)
                            .await
                            .ok()
                            .flatten()
                            .is_some_and(|e| e.is_dir()) =>
                    {
                        dir
                    }
                    _ => session.info.home.clone(),
                };
                let listing = session.fs.list(&start).await;
                Ok(Opened {
                    session,
                    start,
                    listing,
                })
            }
            .await;
            let _ = messages.send(Msg::Connected(Connected {
                epoch,
                site: site.id,
                result,
            }));
        });
        // the local pane follows the site's local directory
        if let Some(dir) = self
            .state
            .servers
            .site(site_id)
            .and_then(|s| s.local_dir.clone())
        {
            self.list_local(dir);
        }
    }

    pub(crate) fn on_connect_step(&mut self, epoch: u64, step: ConnectStep) {
        if epoch == self.connect_epoch
            && let SessionState::Connecting { site, .. } = self.state.session
        {
            self.state.session = SessionState::Connecting { site, step };
            self.changed();
        }
    }

    pub(crate) fn on_connected(&mut self, connected: Connected) {
        if connected.epoch != self.connect_epoch {
            // superseded by a newer connect or a disconnect: close what this one opened
            if let Ok(opened) = connected.result {
                let fs = opened.session.fs;
                tokio::spawn(async move { fs.close().await });
            }
            return;
        }
        match connected.result {
            Ok(opened) => {
                let Opened {
                    session,
                    start,
                    listing,
                } = opened;
                self.live = Some(Live {
                    site: connected.site,
                    fs: session.fs,
                    info: session.info.clone(),
                });
                self.state.session = SessionState::Connected {
                    site: connected.site,
                    info: session.info.clone(),
                };
                self.state.terminal = if session.shell.is_some() {
                    TerminalState::Closed
                } else {
                    TerminalState::NotAvailable
                };
                self.shell = session.shell;
                let mut pane = Pane::new(start);
                match listing {
                    Ok(mut entries) => {
                        retain_visible(&mut entries, self.state.settings.ui.show_hidden);
                        sort_entries(&mut entries, pane.sort);
                        pane.entries = Arc::from(entries);
                        pane.generation = 1;
                    }
                    Err(error) => pane.error = Some(error.to_string()),
                }
                self.state.remote = Some(pane);
                tracing::info!(target: "filecargo::app", "connected");
                self.changed();
                if let Some(command) = self.after_connect.take() {
                    self.handle_command(command);
                }
            }
            Err(error) => {
                self.after_connect = None;
                tracing::warn!(target: "filecargo::app", %error, "connection failed");
                self.state.session = SessionState::Failed {
                    site: connected.site,
                    error: error.to_string(),
                };
                self.changed();
                if needs_a_message(&error) {
                    self.message(Level::Error, "Connection failed", error.to_string());
                }
            }
        }
    }

    /// Closes the current session, if any, and clears everything that belongs to it.
    pub(crate) fn drop_session(&mut self) {
        self.connect_epoch += 1; // a connect still in flight is superseded
        self.after_connect = None;
        self.remote_seq += 1;
        self.shell = None;
        if let Some(live) = self.live.take() {
            let fs = live.fs;
            tokio::spawn(async move { fs.close().await });
        }
        if self.state.remote.take().is_some()
            || !matches!(self.state.session, SessionState::Disconnected)
        {
            self.state.session = SessionState::Disconnected;
            self.state.terminal = TerminalState::NotAvailable;
            self.changed();
        }
    }

    pub(crate) fn disconnect(&mut self) {
        self.drop_session();
        self.changed();
    }

    /// The connection died while browsing: `Failed`, and the next remote command reconnects.
    pub(crate) fn session_lost(&mut self, error: &FsError) {
        if let Some(live) = self.live.take() {
            let site = live.site;
            let fs = live.fs;
            tokio::spawn(async move { fs.close().await });
            tracing::warn!(target: "filecargo::app", %error, "the connection was lost");
            self.state.session = SessionState::Failed {
                site,
                error: error.to_string(),
            };
            self.lost_site = Some(site);
            self.changed();
        }
    }

    /// For a remote command: `true` when it can run now. If the session was lost, reconnects
    /// (once) and replays the command afterwards.
    pub(crate) fn ready_for_remote(&mut self, command: Command) -> Option<Command> {
        if self.live.is_some() {
            return Some(command);
        }
        if let Some(site) = self.lost_site.take()
            && matches!(self.state.session, SessionState::Failed { .. })
        {
            tracing::info!(target: "filecargo::app", "reconnecting after a lost connection");
            self.connect(site, Some(command));
            return None;
        }
        self.notice(Level::Warning, "Not connected to a server.".to_owned());
        None
    }

    // ---- remote pane --------------------------------------------------------------------

    pub(crate) fn list_remote(&mut self, target: RemotePath) {
        let Some(live) = &self.live else { return };
        self.remote_seq += 1;
        let seq = self.remote_seq;
        let fs = live.fs.clone();
        if let Some(pane) = self.state.remote.as_mut() {
            pane.loading = true;
        }
        self.changed();
        let messages = self.messages.clone();
        tokio::spawn(async move {
            let result = fs.list(&target).await.map(|entries| (target, entries));
            let _ = messages.send(Msg::RemoteListed(RemoteListing { seq, result }));
        });
    }

    pub(crate) fn navigate_remote(&mut self, typed: &str) {
        let (Some(pane), Some(live)) = (&self.state.remote, &self.live) else {
            return;
        };
        let typed = typed.trim();
        let combined = if typed == "~" {
            live.info.home.as_str().to_owned()
        } else if let Some(rest) = typed.strip_prefix("~/") {
            format!("{}/{rest}", live.info.home.as_str().trim_end_matches('/'))
        } else if typed.starts_with('/') {
            typed.to_owned()
        } else {
            format!("{}/{typed}", pane.path.as_str().trim_end_matches('/'))
        };
        match RemotePath::parse(&combined) {
            Ok(path) => self.list_remote(path),
            Err(error) => {
                if let Some(pane) = self.state.remote.as_mut() {
                    pane.error = Some(error.to_string());
                }
                self.changed();
            }
        }
    }

    pub(crate) fn up_remote(&mut self) {
        if let Some(parent) = self.state.remote.as_ref().and_then(|p| p.path.parent()) {
            self.list_remote(parent);
        }
    }

    pub(crate) fn refresh_remote(&mut self) {
        if let Some(path) = self.state.remote.as_ref().map(|p| p.path.clone()) {
            self.list_remote(path);
        }
    }

    pub(crate) fn sort_remote(&mut self, sort: crate::state::Sort) {
        if let Some(pane) = self.state.remote.as_mut() {
            let mut entries = pane.entries.to_vec();
            sort_entries(&mut entries, sort);
            pane.sort = sort;
            pane.entries = Arc::from(entries);
            pane.generation += 1;
            self.changed();
        }
    }

    pub(crate) fn on_remote_listed(&mut self, listing: RemoteListing) {
        if listing.seq != self.remote_seq || self.state.remote.is_none() {
            return;
        }
        match listing.result {
            Ok((path, mut entries)) => {
                retain_visible(&mut entries, self.state.settings.ui.show_hidden);
                if let Some(pane) = self.state.remote.as_mut() {
                    sort_entries(&mut entries, pane.sort);
                    pane.path = path;
                    pane.entries = Arc::from(entries);
                    pane.loading = false;
                    pane.error = None;
                    pane.generation += 1;
                }
            }
            Err(error) => {
                if let Some(pane) = self.state.remote.as_mut() {
                    pane.loading = false;
                    pane.error = Some(error.to_string());
                }
                if error.is_retryable() {
                    self.session_lost(&error);
                }
            }
        }
        self.changed();
    }

    /// A credential prompt means authentication is under way.
    pub(crate) fn note_prompt(&mut self, kind: &PromptKind) {
        if matches!(kind, PromptKind::Credential(_))
            && let SessionState::Connecting { site, .. } = self.state.session
        {
            self.state.session = SessionState::Connecting {
                site,
                step: ConnectStep::Authenticating,
            };
        }
    }
}
