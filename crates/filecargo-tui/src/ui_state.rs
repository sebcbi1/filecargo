//! Everything the TUI remembers that `AppState` does not: focus, cursors, selections.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::dialog::Dialog;
use crate::tree::TreeUi;

/// The area keys go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Local,
    Remote,
    Bottom,
}

/// Cursor and selection of one file pane. Row 0 is the `..` row when the directory has a
/// parent (the app's `Pane.entries` never contain it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneUi {
    pub cursor: usize,
    /// First row shown.
    pub offset: usize,
    /// Names of the selected entries (a set of names, so it survives refreshes).
    pub selected: BTreeSet<String>,
    /// `(path, generation)` of the listing this state was last synced with.
    pub seen: Option<(String, u64)>,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub focus: Focus,
    /// Terminal size in cells.
    pub size: (u16, u16),
    pub quit_requested: bool,
    /// `None`: shown when the terminal is at least 100 columns wide.
    pub tree_override: Option<bool>,
    pub maximize_bottom: bool,
    pub tree: TreeUi,
    /// The modal dialog on top of everything, if any; it takes every key.
    pub dialog: Option<Dialog>,
    pub local: PaneUi,
    pub remote: PaneUi,
    /// Whether the last snapshot had a live session (to move the focus when one appears).
    pub was_connected: bool,
    /// For "5 minutes ago"-style dates; set by the loop so drawing stays pure.
    pub now: SystemTime,
    /// For abbreviating paths as `~/...`.
    pub home: Option<PathBuf>,
    /// `false` when `NO_COLOR` is set: emphasis comes from bold / reverse and markers only.
    pub color: bool,
}

impl UiState {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            focus: Focus::Local,
            size: (width, height),
            quit_requested: false,
            tree_override: None,
            maximize_bottom: false,
            tree: TreeUi::default(),
            dialog: None,
            local: PaneUi::default(),
            remote: PaneUi::default(),
            was_connected: false,
            now: SystemTime::now(),
            home: std::env::home_dir(),
            color: std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
        }
    }

    pub fn tree_visible(&self) -> bool {
        self.tree_override.unwrap_or(self.size.0 >= 100)
    }

    pub fn pane(&self, focus: Focus) -> Option<&PaneUi> {
        match focus {
            Focus::Local => Some(&self.local),
            Focus::Remote => Some(&self.remote),
            _ => None,
        }
    }

    pub fn pane_mut(&mut self, focus: Focus) -> Option<&mut PaneUi> {
        match focus {
            Focus::Local => Some(&mut self.local),
            Focus::Remote => Some(&mut self.remote),
            _ => None,
        }
    }
}
