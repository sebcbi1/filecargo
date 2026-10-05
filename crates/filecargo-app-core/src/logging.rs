//! The log both front-ends show: a ring buffer fed by a `tracing` layer.
//!
//! The `log` crate is deliberately **not** bridged: dependencies that log through it (the FTP
//! library traces raw control traffic, passwords included) are compiled with their `log`
//! output off. Everything shown here comes from our own `tracing` events, which are redacted
//! at the source.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use filecargo_config::LogLevel;
use tracing::field::{Field, Visit};
use tracing::{Event, Level as TracingLevel, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

const CAPACITY: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub time: SystemTime,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// Shared by the app and the UI: the UI reads it at render time and compares
/// [`LogBuffer::generation`] to know when to redraw.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Inner>,
}

struct Inner {
    lines: Mutex<VecDeque<LogLine>>,
    generation: AtomicU64,
    /// Events more verbose than this are dropped (`LogLevel` as a number).
    level: AtomicU8,
}

fn rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Error => 0,
        LogLevel::Warn => 1,
        LogLevel::Info => 2,
        LogLevel::Debug => 3,
        LogLevel::Trace => 4,
    }
}

fn from_tracing(level: &TracingLevel) -> LogLevel {
    match *level {
        TracingLevel::ERROR => LogLevel::Error,
        TracingLevel::WARN => LogLevel::Warn,
        TracingLevel::INFO => LogLevel::Info,
        TracingLevel::DEBUG => LogLevel::Debug,
        TracingLevel::TRACE => LogLevel::Trace,
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                lines: Mutex::default(),
                generation: AtomicU64::new(0),
                level: AtomicU8::new(rank(LogLevel::Info)),
            }),
        }
    }
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, line: LogLine) {
        let mut lines = self.inner.lines.lock().unwrap_or_else(|e| e.into_inner());
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
        drop(lines);
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Bumps whenever a line was added.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    pub fn len(&self) -> usize {
        self.inner
            .lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Runs `f` over the lines, oldest first.
    pub fn with_lines<R>(&self, f: impl FnOnce(&VecDeque<LogLine>) -> R) -> R {
        f(&self.inner.lines.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn snapshot(&self) -> Vec<LogLine> {
        self.with_lines(|lines| lines.iter().cloned().collect())
    }

    /// Follows `settings.log.level`: events more verbose than this are dropped.
    pub fn set_level(&self, level: LogLevel) {
        self.inner.level.store(rank(level), Ordering::SeqCst);
    }

    fn accepts(&self, level: LogLevel) -> bool {
        rank(level) <= self.inner.level.load(Ordering::SeqCst)
    }
}

/// Collects the `message` field plus any other fields as `key=value`.
#[derive(Default)]
struct Fields {
    message: String,
    extra: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.extra, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.extra, " {}={value}", field.name());
        }
    }
}

/// A `tracing` layer that feeds a [`LogBuffer`] (and optionally a file).
pub struct LogLayer {
    buffer: LogBuffer,
    file: Option<Mutex<std::fs::File>>,
}

impl LogLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer, file: None }
    }

    /// Also appends every accepted line to `path`. A file that cannot be opened is ignored
    /// (the log stays in memory).
    pub fn with_file(mut self, path: &std::path::Path) -> Self {
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new);
        self
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = from_tracing(event.metadata().level());
        if !self.buffer.accepts(level) {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let line = LogLine {
            time: SystemTime::now(),
            level,
            target: event.metadata().target().to_owned(),
            message: format!("{}{}", fields.message, fields.extra),
        };
        if let Some(file) = &self.file {
            let secs = line
                .time
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(
                file,
                "{secs} {:<5} {}: {}",
                format!("{:?}", line.level).to_uppercase(),
                line.target,
                line.message
            );
        }
        self.buffer.push(line);
    }
}

/// For binaries: makes our `tracing` events feed `buffer` (and the file named by
/// `FILECARGO_LOG_FILE`, if set). Returns `false` when a global subscriber already exists.
pub fn init(buffer: &LogBuffer) -> bool {
    let mut layer = LogLayer::new(buffer.clone());
    if let Some(path) = std::env::var_os("FILECARGO_LOG_FILE").map(PathBuf::from) {
        layer = layer.with_file(&path);
    }
    tracing_subscriber::registry()
        .with(layer)
        .try_init()
        .is_ok()
}
