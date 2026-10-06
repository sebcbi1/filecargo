//! The actor that owns the application: one tokio task, one message queue, one published
//! snapshot.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use filecargo_config::{
    ConfigStore, KeyringStore, Paths, SecretStore, Settings, SiteId, UnavailableStore,
};
use filecargo_remote_fs::{ConnectContext, ShellOpener};
use filecargo_transfer::QueueSnapshot;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::command::Command;
use crate::logging::LogBuffer;
use crate::ops::OpDone;
use crate::pane::LocalListing;
use crate::prompt::{ActorPrompter, PromptAction, PromptRequest};
use crate::session::{Connected, Live, RemoteListing, SessionFactory, default_factory};
use crate::state::{
    AppState, ConnectStep, Level, Notice, NoticeId, Pane, PaneId, Prompt, PromptAnswer, PromptId,
    PromptKind, SessionState, StartError, TerminalState,
};
use crate::terminal::Opened;
use crate::transfers::{RefreshSchedule, Shared};
use filecargo_terminal::{TermStatus, TerminalHandle};
use filecargo_transfer::{Direction, Queue, QueueEvent};

/// Published snapshots are coalesced to at most this many per second.
const SNAPSHOTS_PER_SECOND: u64 = 30;
const MAX_NOTICES: usize = 50;

#[derive(Default)]
pub struct StartOptions {
    /// `None` = `Paths::resolve()` (honors `FILECARGO_CONFIG_DIR`).
    pub paths: Option<Paths>,
    /// `None` = the OS keychain (a warning notice when there is none).
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// `None` = `remote_fs::connect`. Tests serve a directory on disk.
    pub connector: Option<Arc<dyn SessionFactory>>,
}

pub struct App;

impl App {
    /// Creates the runtime, opens the config store and starts the actor. Problems with the
    /// config files do not fail the start: they become `AppState::startup_error`.
    pub fn start(options: StartOptions) -> Result<AppHandle, StartError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("filecargo-core")
            .build()
            .map_err(|e| StartError::Runtime(e.to_string()))?;

        // tokio timers are created while the actor state is built
        let _enter = runtime.enter();
        let mut notices = Vec::new();
        let paths = options
            .paths
            .unwrap_or_else(|| Paths::resolve().unwrap_or_else(|_| Paths::from_override(None)));
        let secrets: Arc<dyn SecretStore> =
            options
                .secrets
                .unwrap_or_else(|| match KeyringStore::native() {
                    Ok(store) => Arc::new(store),
                    Err(error) => {
                        notices.push((
                        Level::Warning,
                        "Passwords cannot be saved on this system: there is no usable keychain."
                            .to_owned(),
                    ));
                        tracing::warn!(target: "filecargo::app", %error, "no keychain available");
                        Arc::new(UnavailableStore::new(error.to_string()))
                    }
                });
        let factory = options.connector.unwrap_or_else(default_factory);

        let (store, startup_error) = match open_store(&paths, &secrets) {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error)),
        };
        let settings = store
            .as_ref()
            .map(|s| s.settings().clone())
            .unwrap_or_default();
        let servers = store.as_ref().map(|s| s.tree().clone()).unwrap_or_default();

        let settings_level = settings.log.level;
        let local_path = start_dir(&settings);
        let state = AppState {
            startup_error,
            servers: Arc::new(servers),
            settings: Arc::new(settings),
            session: SessionState::Disconnected,
            local: Pane::new(local_path),
            remote: None,
            queue: Arc::new(QueueSnapshot::default()),
            terminal: TerminalState::NotAvailable,
            prompt: None,
            notices: Vec::new(),
            log_generation: 0,
        };

        let log = LogBuffer::new();
        log.set_level(settings_level);
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (publisher, snapshots) = watch::channel(Arc::new(state.clone()));
        let shared = Arc::new(Shared {
            tree: Mutex::new(state.servers.clone()),
            timeouts: Mutex::new(state.settings.connection.clone()),
        });
        let mut ctx = ConnectContext::new(
            paths.clone(),
            secrets.clone(),
            Arc::new(ActorPrompter {
                messages: commands_tx.clone(),
            }),
        );
        ctx.timeouts = state.settings.connection.clone();
        let mut core = Core {
            state,
            paths,
            store,
            secrets,
            factory,
            log: log.clone(),
            messages: commands_tx.clone(),
            publisher,
            dirty: false,
            last_publish: Instant::now(),
            prompts: VecDeque::new(),
            replies: HashMap::new(),
            actions: HashMap::new(),
            shared,
            queue: None,
            local_refresh: RefreshSchedule::default(),
            remote_refresh: RefreshSchedule::default(),
            local_seq: 0,
            ctx,
            live: None,
            shell: None,
            terminal: None,
            quitting: false,
            connect_epoch: 0,
            after_connect: None,
            lost_site: None,
            remote_seq: 0,
            next_prompt: 0,
            next_notice: 0,
        };
        for (level, text) in notices {
            core.notice(level, text);
        }
        let handle = runtime.handle().clone();
        handle.spawn(core.run(commands_rx));

        Ok(AppHandle {
            inner: Arc::new(HandleInner {
                messages: commands_tx,
                snapshots,
                log,
                handle,
                runtime: Mutex::new(Some(runtime)),
            }),
        })
    }
}

fn open_store(paths: &Paths, secrets: &Arc<dyn SecretStore>) -> Result<ConfigStore, String> {
    ConfigStore::open(paths.clone(), secrets.clone()).map_err(|e| e.to_string())
}

/// `settings.ui.local_start_dir` with `~` expanded; the home directory when it is unusable.
fn start_dir(settings: &Settings) -> PathBuf {
    let home = std::env::home_dir();
    let configured = settings.ui.local_start_dir.trim();
    let expanded = match (configured.strip_prefix('~'), &home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(configured),
    };
    if !configured.is_empty() && expanded.is_dir() {
        expanded
    } else {
        home.filter(|h| h.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

// Messages are moved once; `Command` is large and other variants will carry results.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Msg {
    Command(Command),
    /// A prompt requested by a background task (a session asking for a password, ...).
    Prompt(PromptRequest),
    /// A local directory listing finished.
    LocalListed(LocalListing),
    RemoteListed(RemoteListing),
    OpDone(OpDone),
    Queue(QueueEvent),
    TerminalOpened(Opened),
    TerminalEnded {
        epoch: u64,
        status: TermStatus,
    },
    /// The connect task found out how far it got.
    ConnectStep {
        epoch: u64,
        step: ConnectStep,
    },
    Connected(Connected),
    Shutdown(oneshot::Sender<()>),
}

struct HandleInner {
    messages: mpsc::UnboundedSender<Msg>,
    snapshots: watch::Receiver<Arc<AppState>>,
    log: LogBuffer,
    handle: tokio::runtime::Handle,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
}

/// What a front-end holds. Cheap to clone.
#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<HandleInner>,
}

impl AppHandle {
    pub fn send(&self, command: Command) {
        let _ = self.inner.messages.send(Msg::Command(command));
    }

    /// The latest snapshot, and a way to wait for the next one.
    pub fn state(&self) -> watch::Receiver<Arc<AppState>> {
        self.inner.snapshots.clone()
    }

    /// The prompter every session uses: prompts show up in `AppState::prompt`. Exposed for
    /// embedders and tests that open sessions themselves.
    pub fn prompter(&self) -> Arc<dyn filecargo_remote_fs::Prompter> {
        Arc::new(ActorPrompter {
            messages: self.inner.messages.clone(),
        })
    }

    /// The shared log ring buffer.
    pub fn log(&self) -> LogBuffer {
        self.inner.log.clone()
    }

    /// The app's tokio runtime, for front-ends that need timers (the TUI runs its loop on it
    /// with `block_on`). The GUI never uses it.
    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.inner.handle.clone()
    }

    /// Flushes the queue, closes sessions and stops the runtime, giving up on anything still
    /// running after `timeout`. Idempotent; call it from outside the runtime.
    pub fn shutdown(self, timeout: Duration) {
        let Some(runtime) = self
            .inner
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            return;
        };
        let (done_tx, done_rx) = oneshot::channel();
        if self.inner.messages.send(Msg::Shutdown(done_tx)).is_ok() {
            let _ = runtime.block_on(async { tokio::time::timeout(timeout, done_rx).await });
        }
        runtime.shutdown_timeout(timeout);
    }
}

pub(crate) struct Core {
    pub(crate) state: AppState,
    pub(crate) paths: Paths,
    pub(crate) store: Option<ConfigStore>,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) factory: Arc<dyn SessionFactory>,
    /// Shared by every connection of the app (browsing and transfers): the in-memory trust
    /// means nobody is asked twice for the same answer.
    pub(crate) ctx: ConnectContext,
    /// The connected session, if any.
    pub(crate) live: Option<Live>,
    pub(crate) shell: Option<ShellOpener>,
    /// The open shell, if any.
    pub(crate) terminal: Option<TerminalHandle>,
    /// Set by a confirmed `Quit`: the actor winds down after the current message.
    pub(crate) quitting: bool,
    /// Bumps on every connect and disconnect; results of older ones are discarded.
    pub(crate) connect_epoch: u64,
    /// Runs once the connection in progress succeeds (an automatic reconnect replays the
    /// command that triggered it).
    pub(crate) after_connect: Option<Command>,
    /// The site whose connection was lost while browsing: the next remote command reconnects.
    pub(crate) lost_site: Option<SiteId>,
    /// Id of the newest remote listing request.
    pub(crate) remote_seq: u64,
    pub(crate) log: LogBuffer,
    /// For tasks that report back to the actor.
    pub(crate) messages: mpsc::UnboundedSender<Msg>,
    publisher: watch::Sender<Arc<AppState>>,
    dirty: bool,
    last_publish: Instant,
    pub(crate) prompts: VecDeque<Prompt>,
    /// Id of the newest local listing request; older results are discarded.
    pub(crate) local_seq: u64,
    /// Where the answer to a prompt goes, for prompts that came from a background task.
    pub(crate) replies: HashMap<PromptId, oneshot::Sender<PromptAnswer>>,
    /// What to do when a prompt the app raised itself is confirmed.
    pub(crate) actions: HashMap<PromptId, PromptAction>,
    /// Sites and timeouts as the queue's workers see them.
    pub(crate) shared: Arc<Shared>,
    pub(crate) queue: Option<Queue>,
    pub(crate) local_refresh: RefreshSchedule,
    pub(crate) remote_refresh: RefreshSchedule,
    next_prompt: u64,
    next_notice: u64,
}

impl Core {
    async fn run(mut self, mut messages: mpsc::UnboundedReceiver<Msg>) {
        let start = self.state.local.path.clone();
        self.list_local(start);
        self.start_queue().await;
        let interval = Duration::from_millis(1000 / SNAPSHOTS_PER_SECOND);
        loop {
            let mut wake = if self.dirty {
                self.last_publish + interval
            } else {
                Instant::now() + Duration::from_secs(3600)
            };
            if let Some(due) = self.next_refresh_due() {
                wake = wake.min(due);
            }
            tokio::select! {
                message = messages.recv() => match message {
                    Some(Msg::Command(command)) => self.handle_command(command),
                    Some(Msg::Prompt(request)) => self.request_prompt(request),
                    Some(Msg::LocalListed(listing)) => self.on_local_listed(listing),
                    Some(Msg::RemoteListed(listing)) => self.on_remote_listed(listing),
                    Some(Msg::OpDone(done)) => self.on_op_done(done),
                    Some(Msg::Queue(event)) => self.on_queue_event(event),
                    Some(Msg::TerminalOpened(opened)) => self.on_terminal_opened(opened),
                    Some(Msg::TerminalEnded { epoch, status }) => self.on_terminal_ended(epoch, status),
                    Some(Msg::Connected(connected)) => self.on_connected(connected),
                    Some(Msg::ConnectStep { epoch, step }) => self.on_connect_step(epoch, step),
                    Some(Msg::Shutdown(ack)) => {
                        self.shutdown().await;
                        self.publish_now();
                        let _ = ack.send(());
                        return;
                    }
                    None => return,
                },
                () = tokio::time::sleep_until(wake) => {}
            }
            self.run_due_refreshes();
            if self.quitting {
                self.shutdown().await;
                self.publish_now();
                return;
            }
            if self.dirty && self.last_publish.elapsed() >= interval {
                self.publish_now();
            }
        }
    }

    /// Marks the state as changed; the actor publishes it (coalesced).
    pub(crate) fn changed(&mut self) {
        self.dirty = true;
    }

    fn publish_now(&mut self) {
        self.state.log_generation = self.log.generation();
        self.publisher.send_replace(Arc::new(self.state.clone()));
        self.dirty = false;
        self.last_publish = Instant::now();
    }

    /// Persists the queue and closes the shell and the session.
    async fn shutdown(&mut self) {
        if let Some(queue) = self.queue.take() {
            queue.shutdown().await;
        }
        self.close_terminal();
        if let Some(live) = self.live.take() {
            live.fs.close().await;
        }
        self.shell = None;
        self.state.session = SessionState::Disconnected;
        self.state.remote = None;
        self.state.terminal = TerminalState::NotAvailable;
        self.changed();
    }

    /// `Quit`: asks first when transfers are running (they would be interrupted; the queue is
    /// saved either way).
    fn request_quit(&mut self) {
        let active = self
            .state
            .queue
            .pending
            .iter()
            .filter(|v| matches!(v.item.state, filecargo_transfer::ItemState::Active { .. }))
            .count();
        if active > 0 {
            let id = self.enqueue_prompt(PromptKind::ConfirmQuit {
                active_transfers: active,
            });
            self.actions.insert(id, PromptAction::Quit);
        } else {
            self.quitting = true;
        }
    }

    pub(crate) fn notice(&mut self, level: Level, text: String) {
        self.next_notice += 1;
        self.state.notices.push(Notice {
            id: NoticeId(self.next_notice),
            level,
            text,
        });
        let excess = self.state.notices.len().saturating_sub(MAX_NOTICES);
        self.state.notices.drain(..excess);
        self.changed();
    }

    /// Queues a prompt behind any that are showing.
    pub(crate) fn enqueue_prompt(&mut self, kind: PromptKind) -> PromptId {
        self.next_prompt += 1;
        let id = PromptId(self.next_prompt);
        self.prompts.push_back(Prompt { id, kind });
        self.sync_prompt();
        id
    }

    /// The front of the queue is what the UI shows.
    pub(crate) fn sync_prompt(&mut self) {
        self.state.prompt = self.prompts.front().cloned();
        self.changed();
    }

    pub(crate) fn message(
        &mut self,
        level: Level,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        self.enqueue_prompt(PromptKind::Message {
            level,
            title: title.into(),
            body: body.into(),
        });
    }

    pub(crate) fn handle_command(&mut self, command: Command) {
        match command {
            Command::ResetConfig => self.reset_config(),
            Command::Tree(op) => self.apply_tree_op(op),
            Command::ImportFileZilla {
                path,
                import_passwords,
            } => {
                self.import_filezilla(&path, import_passwords);
            }
            Command::SetSitePassword { site, secret } => self.set_site_password(site, &secret),
            Command::UpdateSettings(settings) => self.update_settings(settings),
            Command::Answer { id, answer } => self.answer_prompt(id, answer),
            Command::Navigate {
                pane: PaneId::Local,
                path,
            } => self.navigate_local(&path),
            command @ (Command::Mkdir { .. }
            | Command::Rename { .. }
            | Command::Delete { .. }
            | Command::Chmod { .. }) => self.file_operation(command),
            Command::Upload { names } => self.start_transfers(Direction::Upload, &names),
            Command::Download { names } => self.start_transfers(Direction::Download, &names),
            command @ (Command::QueueRetry(_)
            | Command::QueueRetryFailed
            | Command::QueueRemove(_)
            | Command::QueueClearCompleted
            | Command::QueueSetProcessing(_)) => self.queue_command(command),
            command @ (Command::TerminalOpen { .. }
            | Command::TerminalInput(_)
            | Command::TerminalResize { .. }
            | Command::TerminalScroll(_)
            | Command::TerminalClose) => self.terminal_command(command),
            Command::Quit => self.request_quit(),
            Command::Connect(site) => self.connect(site, None),
            Command::Disconnect => self.disconnect(),
            Command::SetSort {
                pane: PaneId::Remote,
                sort,
            } => self.sort_remote(sort),
            command @ (Command::Navigate {
                pane: PaneId::Remote,
                ..
            }
            | Command::Up(PaneId::Remote)
            | Command::Refresh(PaneId::Remote)) => {
                if let Some(command) = self.ready_for_remote(command) {
                    match command {
                        Command::Navigate { path, .. } => self.navigate_remote(&path),
                        Command::Up(_) => self.up_remote(),
                        Command::Refresh(_) => self.refresh_remote(),
                        _ => {}
                    }
                }
            }
            Command::Up(PaneId::Local) => self.up_local(),
            Command::Refresh(PaneId::Local) => self.refresh_local(),
            Command::SetSort {
                pane: PaneId::Local,
                sort,
            } => self.sort_local(sort),
            Command::DismissNotice(id) => {
                self.state.notices.retain(|n| n.id != id);
                self.changed();
            }
        }
    }

    // ---- config -----------------------------------------------------------------------------

    fn adopt_store(&mut self, store: ConfigStore) {
        self.state.servers = Arc::new(store.tree().clone());
        self.state.settings = Arc::new(store.settings().clone());
        self.state.startup_error = None;
        self.store = Some(store);
        self.sync_shared();
        self.changed();
    }

    /// Backs up the unreadable config file(s) and reloads.
    fn reset_config(&mut self) {
        let mut backups = Vec::new();
        match ConfigStore::reset(&self.paths) {
            Ok(Some(backup)) => backups.push(backup),
            Ok(None) => {}
            Err(error) => {
                self.message(
                    Level::Error,
                    "Cannot reset the configuration",
                    error.to_string(),
                );
                return;
            }
        }
        let mut opened = open_store(&self.paths, &self.secrets);
        if opened.is_err() {
            // `ConfigStore::reset` only moves `servers.toml`; an unreadable `settings.toml`
            // is set aside the same way.
            let settings = self.paths.settings();
            if settings.exists() {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let backup = settings.with_extension(format!("toml.bak-{secs}"));
                if std::fs::rename(&settings, &backup).is_ok() {
                    backups.push(backup);
                    opened = open_store(&self.paths, &self.secrets);
                }
            }
        }
        match opened {
            Ok(store) => {
                self.adopt_store(store);
                let list = backups
                    .iter()
                    .map(|b| b.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let text = if backups.is_empty() {
                    "The configuration was reloaded.".to_owned()
                } else {
                    format!("The unreadable configuration was moved to {list}.")
                };
                self.notice(Level::Info, text);
            }
            Err(error) => {
                self.state.startup_error = Some(error);
                self.changed();
            }
        }
    }

    fn update_settings(&mut self, settings: Settings) {
        let Some(store) = self.store.as_mut() else {
            self.message(
                Level::Error,
                "Settings not saved",
                "The configuration could not be loaded.",
            );
            return;
        };
        match store.update_settings(|current| *current = settings) {
            Ok(()) => {
                let hidden_before = self.state.settings.ui.show_hidden;
                self.state.settings = Arc::new(store.settings().clone());
                self.log.set_level(self.state.settings.log.level);
                self.sync_shared();
                if let Some(queue) = &self.queue {
                    queue.set_limits(self.queue_limits());
                }
                self.changed();
                if self.state.settings.ui.show_hidden != hidden_before {
                    // what is listed depends on it
                    self.refresh_local();
                }
            }
            Err(error) => self.message(Level::Error, "Settings not saved", error.to_string()),
        }
    }
}
