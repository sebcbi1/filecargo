//! The log both front-ends show: a ring buffer fed by a `tracing` layer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::state::Level;

const CAPACITY: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub time: SystemTime,
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// Shared by the app and the UI: the UI reads it at render time and compares
/// [`LogBuffer::generation`] to know when to redraw.
#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    lines: Mutex<VecDeque<LogLine>>,
    generation: AtomicU64,
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
}
