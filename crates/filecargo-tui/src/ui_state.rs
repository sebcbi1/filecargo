//! Everything the TUI remembers that `AppState` does not: focus, cursors, dialogs.

/// The area keys go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Local,
    Remote,
    Bottom,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub focus: Focus,
    /// Terminal size in cells.
    pub size: (u16, u16),
    pub quit_requested: bool,
}

impl UiState {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            focus: Focus::Local,
            size: (width, height),
            quit_requested: false,
        }
    }
}
