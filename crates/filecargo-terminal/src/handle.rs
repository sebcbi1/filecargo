//! The emulator a UI draws and types into: a `vt100` parser fed by a shell channel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use filecargo_remote_fs::{ShellChannel, ShellInput, ShellOutput};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::keys::{Key, Modes, Mods, encode};

/// Resizes reach the server at most this often; the latest size wins.
const RESIZE_INTERVAL: Duration = Duration::from_millis(100);
/// Output is fed to the parser in chunks of at most this size, so the UI can take the lock
/// between them while a `cat bigfile` runs.
const FEED_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermStatus {
    Running,
    /// The shell ended. The screen stays readable; input is ignored.
    Exited(Option<u32>),
    /// The channel went away without an exit status.
    Closed,
}

/// Collects what `vt100` reports through callbacks instead of the screen.
#[derive(Default)]
pub(crate) struct TermCallbacks {
    title: String,
    bells: u64,
}

impl vt100::Callbacks for TermCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bells += 1;
    }
    fn visual_bell(&mut self, _: &mut vt100::Screen) {
        self.bells += 1;
    }
}

struct Shared {
    parser: Mutex<vt100::Parser<TermCallbacks>>,
    generation: AtomicU64,
    status: Mutex<TermStatus>,
}

impl Shared {
    fn parser(&self) -> MutexGuard<'_, vt100::Parser<TermCallbacks>> {
        // A panic while drawing must not take the terminal down with it.
        self.parser.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn status(&self) -> TermStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_status(&self, status: TermStatus) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
        self.bump();
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct TerminalHandle {
    shared: Arc<Shared>,
    input: mpsc::UnboundedSender<ShellInput>,
    resize: watch::Sender<TermSize>,
}

/// Starts the emulator on an already-opened channel. Must be called inside a tokio runtime:
/// it spawns one task that feeds output into the parser and forwards resizes, and ends when
/// the channel closes.
pub fn spawn(channel: ShellChannel, size: TermSize, scrollback: usize) -> TerminalHandle {
    let shared = Arc::new(Shared {
        parser: Mutex::new(vt100::Parser::new_with_callbacks(
            size.rows,
            size.cols,
            scrollback,
            TermCallbacks::default(),
        )),
        generation: AtomicU64::new(0),
        status: Mutex::new(TermStatus::Running),
    });
    let (resize_tx, resize_rx) = watch::channel(size);
    let ShellChannel { input, output } = channel;
    tokio::spawn(feeder(shared.clone(), output, input.clone(), resize_rx));
    TerminalHandle {
        shared,
        input,
        resize: resize_tx,
    }
}

async fn feeder(
    shared: Arc<Shared>,
    mut output: mpsc::UnboundedReceiver<ShellOutput>,
    input: mpsc::UnboundedSender<ShellInput>,
    mut resize: watch::Receiver<TermSize>,
) {
    let mut last_sent: Option<Instant> = None;
    let mut deferred: Option<Instant> = None;
    loop {
        let due = async {
            match deferred {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            message = output.recv() => match message {
                Some(ShellOutput::Data(bytes)) => {
                    for chunk in bytes.chunks(FEED_CHUNK) {
                        shared.parser().process(chunk);
                        shared.bump();
                    }
                }
                Some(ShellOutput::Exit(code)) => shared.set_status(TermStatus::Exited(code)),
                Some(ShellOutput::Closed) | None => {
                    if shared.status() == TermStatus::Running {
                        shared.set_status(TermStatus::Closed);
                    }
                    return;
                }
            },
            changed = resize.changed() => {
                if changed.is_err() {
                    continue;
                }
                let now = Instant::now();
                match last_sent {
                    Some(at) if now.duration_since(at) < RESIZE_INTERVAL => {
                        deferred = Some(at + RESIZE_INTERVAL);
                    }
                    _ => {
                        send_resize(&input, *resize.borrow_and_update());
                        last_sent = Some(now);
                        deferred = None;
                    }
                }
            },
            () = due => {
                send_resize(&input, *resize.borrow_and_update());
                last_sent = Some(Instant::now());
                deferred = None;
            },
        }
    }
}

fn send_resize(input: &mpsc::UnboundedSender<ShellInput>, size: TermSize) {
    let _ = input.send(ShellInput::Resize {
        cols: size.cols,
        rows: size.rows,
    });
}

impl TerminalHandle {
    /// Runs `f` with the current screen. Hold the lock only while drawing.
    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R {
        f(self.shared.parser().screen())
    }

    fn running(&self) -> bool {
        self.shared.status() == TermStatus::Running
    }

    fn modes(&self) -> Modes {
        self.with_screen(|s| Modes {
            application_cursor: s.application_cursor(),
            application_keypad: s.application_keypad(),
        })
    }

    /// Typing returns the view to the live screen.
    fn to_live(&self) {
        let mut parser = self.shared.parser();
        if parser.screen().scrollback() != 0 {
            parser.screen_mut().set_scrollback(0);
            self.shared.bump();
        }
    }

    fn send_bytes(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() && self.running() {
            self.to_live();
            let _ = self.input.send(ShellInput::Data(bytes));
        }
    }

    /// Encoded with the screen's current modes (application cursor keys, ...).
    pub fn send_key(&self, key: Key, mods: Mods) {
        self.send_bytes(encode(key, mods, self.modes()));
    }

    /// Typed text (UTF-8), sent as is.
    pub fn send_text(&self, text: &str) {
        self.send_bytes(text.as_bytes().to_vec());
    }

    /// Bracketed when the remote application asked for it; otherwise line breaks become `\r`,
    /// as if typed. The end-of-paste marker is stripped from the text, so pasted content cannot
    /// close the bracket early and have the rest run as typed input.
    pub fn paste(&self, text: &str) {
        let bracketed = self.with_screen(vt100::Screen::bracketed_paste);
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
        self.send_bytes(bytes);
    }

    /// Resizes the screen now and tells the server soon (at most 10 times a second; the latest
    /// size wins).
    pub fn resize(&self, size: TermSize) {
        {
            let mut parser = self.shared.parser();
            parser.screen_mut().set_size(size.rows, size.cols);
        }
        self.shared.bump();
        self.resize.send_replace(size);
    }

    /// Scrolls the view `lines` back into history (negative: forward); `0` returns to the live
    /// view.
    pub fn scroll(&self, lines: i32) {
        let mut parser = self.shared.parser();
        let screen = parser.screen_mut();
        let target = if lines == 0 {
            0
        } else {
            (screen.scrollback() as i64 + i64::from(lines)).max(0) as usize
        };
        screen.set_scrollback(target);
        drop(parser);
        self.shared.bump();
    }

    /// Text between two `(row, col)` cells of the visible screen, for copying. The order of
    /// the two ends does not matter.
    pub fn selection_text(&self, from: (u16, u16), to: (u16, u16)) -> String {
        let (start, end) = if from <= to { (from, to) } else { (to, from) };
        self.with_screen(|s| s.contents_between(start.0, start.1, end.0, end.1))
    }

    pub fn status(&self) -> TermStatus {
        self.shared.status()
    }

    /// Bumps whenever the screen (or the scroll position, size or status) changes.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }

    pub fn title(&self) -> String {
        self.shared.parser().callbacks().title.clone()
    }

    /// How many times the remote rang the bell; a UI flashes when it changes.
    pub fn bell_count(&self) -> u64 {
        self.shared.parser().callbacks().bells
    }

    /// Asks the server to end the shell.
    pub fn close(&self) {
        let _ = self.input.send(ShellInput::Close);
    }
}
