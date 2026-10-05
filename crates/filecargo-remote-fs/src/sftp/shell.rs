//! Interactive shell on the SSH connection that already carries the SFTP session.

use std::sync::Arc;

use russh::ChannelMsg;
use tokio::sync::mpsc;

use super::SshConnection;
use crate::FsError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellInput {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellOutput {
    /// Terminal bytes (stdout and stderr arrive interleaved, as on a real terminal).
    Data(Vec<u8>),
    /// The shell ended; `None` when the server reported no status (killed by a signal).
    Exit(Option<u32>),
    /// Always the last message.
    Closed,
}

pub struct ShellChannel {
    pub input: mpsc::UnboundedSender<ShellInput>,
    pub output: mpsc::UnboundedReceiver<ShellOutput>,
}

/// Something that can open an interactive shell. The SFTP session provides one over its SSH
/// connection; embedders and tests can provide their own.
#[async_trait::async_trait]
pub trait ShellBackend: Send + Sync {
    async fn open(&self, term: &str, cols: u16, rows: u16) -> Result<ShellChannel, FsError>;
}

/// Opens shells for a session. Cheap to clone.
#[derive(Clone)]
pub struct ShellOpener {
    backend: Arc<dyn ShellBackend>,
}

/// A shell on the SFTP session's own SSH connection.
struct SshShell {
    conn: Arc<SshConnection>,
}

impl std::fmt::Debug for ShellOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellOpener").finish_non_exhaustive()
    }
}

fn disconnected(e: impl std::fmt::Display) -> FsError {
    FsError::Disconnected(e.to_string())
}

impl ShellOpener {
    pub(crate) fn new(conn: Arc<SshConnection>) -> Self {
        Self {
            backend: Arc::new(SshShell { conn }),
        }
    }

    /// A shell opener backed by something other than a real SSH connection (tests, embedders).
    pub fn from_backend(backend: Arc<dyn ShellBackend>) -> Self {
        Self { backend }
    }

    /// Opens a PTY and a shell, on the **same** SSH connection as the SFTP session for real
    /// sessions.
    pub async fn open(&self, term: &str, cols: u16, rows: u16) -> Result<ShellChannel, FsError> {
        self.backend.open(term, cols, rows).await
    }
}

#[async_trait::async_trait]
impl ShellBackend for SshShell {
    async fn open(&self, term: &str, cols: u16, rows: u16) -> Result<ShellChannel, FsError> {
        let mut channel = self
            .conn
            .handle
            .channel_open_session()
            .await
            .map_err(disconnected)?;
        channel
            .request_pty(true, term, u32::from(cols), u32::from(rows), 0, 0, &[])
            .await
            .map_err(disconnected)?;
        await_reply(&mut channel, "pty").await?;
        channel.request_shell(true).await.map_err(disconnected)?;
        await_reply(&mut channel, "shell").await?;

        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<ShellInput>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<ShellOutput>();
        tokio::spawn(async move {
            let mut exit = None;
            let mut input_open = true;
            // The loop always drains the channel: russh's session loop blocks on a full channel
            // buffer, so an unread shell would stall the SFTP traffic on the same connection.
            loop {
                tokio::select! {
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                            let _ = output_tx.send(ShellOutput::Data(data.to_vec()));
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                        Some(ChannelMsg::Close) | None => break,
                        Some(_) => {}
                    },
                    command = input_rx.recv(), if input_open => match command {
                        Some(ShellInput::Data(bytes)) => {
                            if channel.data_bytes(bytes).await.is_err() {
                                break;
                            }
                        }
                        Some(ShellInput::Resize { cols, rows }) => {
                            let _ = channel.window_change(u32::from(cols), u32::from(rows), 0, 0).await;
                        }
                        Some(ShellInput::Close) | None => {
                            input_open = false;
                            let _ = channel.eof().await;
                            let _ = channel.close().await;
                        }
                    },
                }
            }
            let _ = output_tx.send(ShellOutput::Exit(exit));
            let _ = output_tx.send(ShellOutput::Closed);
        });
        Ok(ShellChannel {
            input: input_tx,
            output: output_rx,
        })
    }
}

async fn await_reply(
    channel: &mut russh::Channel<russh::client::Msg>,
    what: &str,
) -> Result<(), FsError> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => {
                return Err(FsError::Protocol {
                    code: None,
                    message: format!("the server refused the {what} request"),
                });
            }
            Some(ChannelMsg::Close) | None => {
                return Err(FsError::Disconnected(format!(
                    "channel closed during {what} request"
                )));
            }
            Some(_) => {}
        }
    }
}
