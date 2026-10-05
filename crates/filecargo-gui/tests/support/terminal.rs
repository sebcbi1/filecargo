#![allow(unused_imports)]
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! A fake shell for sessions served by the test factory (adapted from app-core's test support).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filecargo_remote_fs::FsError;
use filecargo_remote_fs::{ShellBackend, ShellChannel, ShellInput, ShellOutput};
use tokio::sync::mpsc;

/// The "server" end of a shell the app opened.
pub struct ShellRemote {
    pub output: mpsc::UnboundedSender<ShellOutput>,
    pub input: mpsc::UnboundedReceiver<ShellInput>,
}

impl ShellRemote {
    pub fn say(&self, text: &str) {
        self.output
            .send(ShellOutput::Data(text.as_bytes().to_vec()))
            .unwrap();
    }

    pub fn received(&mut self) -> Vec<ShellInput> {
        let mut all = Vec::new();
        while let Ok(message) = self.input.try_recv() {
            all.push(message);
        }
        all
    }
}

#[derive(Default)]
pub struct FakeShell {
    pub opened: Mutex<Vec<(String, u16, u16)>>,
    remotes: Mutex<Vec<ShellRemote>>,
    pub fail_next: AtomicBool,
}

impl FakeShell {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn opened(&self) -> usize {
        self.opened.lock().unwrap().len()
    }

    /// The server end of the oldest shell not yet taken.
    pub fn take_remote(&self) -> ShellRemote {
        for _ in 0..200 {
            if let Some(remote) = {
                let mut remotes = self.remotes.lock().unwrap();
                (!remotes.is_empty()).then(|| remotes.remove(0))
            } {
                return remote;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no shell was opened");
    }
}

#[async_trait]
impl ShellBackend for FakeShell {
    async fn open(&self, term: &str, cols: u16, rows: u16) -> Result<ShellChannel, FsError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(FsError::PermissionDenied(
                "shell access is disabled".to_owned(),
            ));
        }
        self.opened
            .lock()
            .unwrap()
            .push((term.to_owned(), cols, rows));
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        self.remotes.lock().unwrap().push(ShellRemote {
            output: output_tx,
            input: input_rx,
        });
        Ok(ShellChannel {
            input: input_tx,
            output: output_rx,
        })
    }
}
