//! Everything the TUI remembers that `AppState` does not: focus, cursors, selections.

use std::collections::BTreeSet;

use filecargo_app_core::prelude::LogBuffer;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::dialog::Dialog;
use crate::prompt_ui::PromptSlot;
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

/// The tabs of the bottom panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Queue,
    Completed,
    Failed,
    Log,
    Terminal,
}

impl BottomTab {
    pub const ALL: [Self; 5] = [
        Self::Queue,
        Self::Completed,
        Self::Failed,
        Self::Log,
        Self::Terminal,
    ];

    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(usize::from(index)).copied()
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Log => "Log",
            Self::Terminal => "Terminal",
        }
    }
}

/// Cursor of one list tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListUi {
    pub cursor: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BottomUi {
    pub tab: BottomTab,
    pub queue: ListUi,
    pub completed: ListUi,
    pub failed: ListUi,
    /// Lines scrolled up from the newest; 0 while following.
    pub log_scroll: usize,
    pub log_follow: bool,
    /// Size last given to the shell, to send a resize only when it changes.
    pub term_size: Option<(u16, u16)>,
    /// An open request is out, so the next frame does not send another.
    pub term_open_sent: bool,
}

impl Default for BottomUi {
    fn default() -> Self {
        Self {
            tab: BottomTab::Queue,
            queue: ListUi::default(),
            completed: ListUi::default(),
            failed: ListUi::default(),
            log_scroll: 0,
            log_follow: true,
            term_size: None,
            term_open_sent: false,
        }
    }
}

impl BottomUi {
    pub fn list_mut(&mut self) -> Option<&mut ListUi> {
        match self.tab {
            BottomTab::Queue => Some(&mut self.queue),
            BottomTab::Completed => Some(&mut self.completed),
            BottomTab::Failed => Some(&mut self.failed),
            BottomTab::Log | BottomTab::Terminal => None,
        }
    }
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
    /// The app's prompt on show (it sits above any dialog).
    pub prompt: Option<PromptSlot>,
    pub bottom: BottomUi,
    /// The app's log, read when the Log tab draws.
    pub log: LogBuffer,
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
            prompt: None,
            bottom: BottomUi::default(),
            log: LogBuffer::new(),
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
