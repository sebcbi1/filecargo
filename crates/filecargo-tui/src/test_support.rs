//! Deterministic fixtures for reducer and snapshot tests: fixed names, sizes and times.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use filecargo_app_core::prelude::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::reducer;
use crate::ui_state::UiState;
use crate::view;

/// 2026-09-30 12:00 UTC.
pub const NOW: u64 = 1_790_769_600;

pub fn entry(name: &str, dir: bool, size: u64, hours_ago: u64) -> Entry {
    let mut e = Entry::new(name, if dir { EntryKind::Dir } else { EntryKind::File });
    e.size = if dir { 0 } else { size };
    e.modified = Some(UNIX_EPOCH + Duration::from_secs(NOW - hours_ago * 3600));
    e
}

pub fn local_entries() -> Vec<Entry> {
    vec![
        entry("docs", true, 0, 30),
        entry("src", true, 0, 50),
        entry("README.md", false, 2150, 26),
        entry("notes.txt", false, 340, 3),
        entry("big.iso", false, 1_600_000_000, 900),
    ]
}

pub fn remote_entries() -> Vec<Entry> {
    vec![
        entry("html", true, 0, 40),
        entry("index.php", false, 4400, 38),
        entry("style.css", false, 12_800, 38),
    ]
}

pub fn pane<P>(path: P, entries: Vec<Entry>) -> Pane<P> {
    Pane {
        path,
        entries: Arc::from(entries),
        sort: Sort::default(),
        loading: false,
        error: None,
        generation: 1,
    }
}

/// Disconnected, local pane at `/home/me/projects`.
pub fn app() -> AppState {
    AppState {
        startup_error: None,
        servers: Arc::new(ServerTree::default()),
        settings: Arc::new(Settings::default()),
        session: SessionState::Disconnected,
        local: pane(PathBuf::from("/home/me/projects"), local_entries()),
        remote: None,
        queue: Arc::new(QueueSnapshot::default()),
        terminal: TerminalState::NotAvailable,
        prompt: None,
        notices: Vec::new(),
        log_generation: 0,
    }
}

/// The same, connected to a server with a remote pane at `/var/www`.
pub fn connected_app() -> AppState {
    let mut app = app();
    app.remote = Some(pane(
        RemotePath::parse("/var/www").unwrap(),
        remote_entries(),
    ));
    app
}

pub fn ui(width: u16, height: u16) -> UiState {
    let mut ui = UiState::new(width, height);
    ui.now = UNIX_EPOCH + Duration::from_secs(NOW);
    ui.home = Some(PathBuf::from("/home/me"));
    ui.color = true;
    ui
}

/// A synced UI for `app`.
pub fn synced(width: u16, height: u16, app: &AppState) -> UiState {
    let mut ui = ui(width, height);
    reducer::sync(&mut ui, app);
    ui
}

/// The screen as text, for snapshots.
pub fn draw(ui: &UiState, app: &AppState) -> String {
    let (width, height) = ui.size;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view::render(frame, ui, app)).unwrap();
    terminal.backend().to_string()
}
