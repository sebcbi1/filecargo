//! Every key binding, in one table. The reducer looks keys up here and the help overlay draws
//! from here, so the two cannot drift apart.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a binding applies. A key is looked up in the focused context first, then in `Global`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Global,
    Tree,
    Files,
    Queue,
    Log,
    Terminal,
}

/// What a binding does. The reducer turns these into commands and UI changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // global
    FocusNext,
    FocusPrev,
    Help,
    Refresh,
    BottomTab(u8),
    ToggleTree,
    MaximizeBottom,
    Quit,
    // lists (tree, panes, queue, log)
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    // file panes
    Open,
    Parent,
    ToggleSelect,
    InvertSelection,
    SelectAll,
    Transfer,
    MakeDir,
    Rename,
    Delete,
    Chmod,
    ToggleHidden,
    CycleSort,
    GoTo,
    // server tree
    Expand,
    Collapse,
    Disconnect,
    DuplicateSite,
    NewSite,
    NewFolder,
    EditSite,
    RenameNode,
    MoveNode,
    DeleteNode,
    ImportFileZilla,
    // bottom panel
    TabNext,
    TabPrev,
    QueuePause,
    QueueRemove,
    QueueRetry,
    QueueRetryAll,
    QueueClear,
    LogFollow,
    // terminal tab
    TerminalEscape,
    TerminalScrollUp,
    TerminalScrollDown,
}

#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

pub struct Binding {
    pub context: Context,
    pub keys: &'static [KeySpec],
    /// How the keys are shown in the help overlay.
    pub label: &'static str,
    pub action: Action,
    pub help: &'static str,
}

const fn plain(code: KeyCode) -> KeySpec {
    KeySpec {
        code,
        mods: KeyModifiers::NONE,
    }
}

const fn ch(c: char) -> KeySpec {
    plain(KeyCode::Char(c))
}

const fn ctrl(c: char) -> KeySpec {
    KeySpec {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

const fn alt(c: char) -> KeySpec {
    KeySpec {
        code: KeyCode::Char(c),
        mods: KeyModifiers::ALT,
    }
}

const fn shifted(code: KeyCode) -> KeySpec {
    KeySpec {
        code,
        mods: KeyModifiers::SHIFT,
    }
}

const fn f(n: u8) -> KeySpec {
    plain(KeyCode::F(n))
}

use Action as A;
use Context as C;

#[rustfmt::skip]
pub static BINDINGS: &[Binding] = &[
    // ---- global ----------------------------------------------------------------------------
    Binding { context: C::Global, keys: &[plain(KeyCode::Tab)], label: "Tab", action: A::FocusNext, help: "focus the next area" },
    Binding { context: C::Global, keys: &[plain(KeyCode::BackTab), shifted(KeyCode::Tab)], label: "Shift-Tab", action: A::FocusPrev, help: "focus the previous area" },
    Binding { context: C::Global, keys: &[f(1), ch('?')], label: "F1 ?", action: A::Help, help: "this help" },
    Binding { context: C::Global, keys: &[ctrl('r')], label: "Ctrl-r", action: A::Refresh, help: "refresh the focused pane" },
    Binding { context: C::Global, keys: &[alt('1')], label: "Alt-1", action: A::BottomTab(0), help: "bottom tab: Queue" },
    Binding { context: C::Global, keys: &[alt('2')], label: "Alt-2", action: A::BottomTab(1), help: "bottom tab: Completed" },
    Binding { context: C::Global, keys: &[alt('3')], label: "Alt-3", action: A::BottomTab(2), help: "bottom tab: Failed" },
    Binding { context: C::Global, keys: &[alt('4')], label: "Alt-4", action: A::BottomTab(3), help: "bottom tab: Log" },
    Binding { context: C::Global, keys: &[alt('5')], label: "Alt-5", action: A::BottomTab(4), help: "bottom tab: Terminal" },
    Binding { context: C::Global, keys: &[f(9)], label: "F9", action: A::ToggleTree, help: "show / hide the server tree" },
    Binding { context: C::Global, keys: &[f(10)], label: "F10", action: A::MaximizeBottom, help: "maximize / restore the bottom panel" },
    Binding { context: C::Global, keys: &[ch('q'), ctrl('q')], label: "q Ctrl-q", action: A::Quit, help: "quit" },
    // ---- file panes ---------------------------------------------------------------------------
    Binding { context: C::Files, keys: &[plain(KeyCode::Up), ch('k')], label: "Up Down", action: A::Up, help: "move the cursor" },
    Binding { context: C::Files, keys: &[plain(KeyCode::Down), ch('j')], label: "", action: A::Down, help: "" },
    Binding { context: C::Files, keys: &[plain(KeyCode::PageUp)], label: "PgUp PgDn", action: A::PageUp, help: "page up / down" },
    Binding { context: C::Files, keys: &[plain(KeyCode::PageDown)], label: "", action: A::PageDown, help: "" },
    Binding { context: C::Files, keys: &[plain(KeyCode::Home)], label: "Home End", action: A::Home, help: "first / last entry" },
    Binding { context: C::Files, keys: &[plain(KeyCode::End)], label: "", action: A::End, help: "" },
    Binding { context: C::Files, keys: &[plain(KeyCode::Enter)], label: "Enter", action: A::Open, help: "open a folder, or transfer a file to the other side" },
    Binding { context: C::Files, keys: &[plain(KeyCode::Backspace)], label: "Backspace", action: A::Parent, help: "parent folder" },
    Binding { context: C::Files, keys: &[ch(' '), plain(KeyCode::Insert)], label: "Space Ins", action: A::ToggleSelect, help: "select / unselect, move down" },
    Binding { context: C::Files, keys: &[ch('*')], label: "*", action: A::InvertSelection, help: "invert the selection" },
    Binding { context: C::Files, keys: &[ctrl('a')], label: "Ctrl-a", action: A::SelectAll, help: "select all" },
    Binding { context: C::Files, keys: &[f(5), ch('t')], label: "F5 t", action: A::Transfer, help: "transfer the selection (or the cursor row) to the other side" },
    Binding { context: C::Files, keys: &[f(7)], label: "F7", action: A::MakeDir, help: "new folder (current pane)" },
    Binding { context: C::Files, keys: &[f(2)], label: "F2", action: A::Rename, help: "rename (current pane)" },
    Binding { context: C::Files, keys: &[f(8), plain(KeyCode::Delete)], label: "F8 Del", action: A::Delete, help: "delete (current pane)" },
    Binding { context: C::Files, keys: &[ch('c')], label: "c", action: A::Chmod, help: "change permissions (current pane)" },
    Binding { context: C::Files, keys: &[ch('.')], label: ".", action: A::ToggleHidden, help: "show / hide dotfiles" },
    Binding { context: C::Files, keys: &[ch('s')], label: "s", action: A::CycleSort, help: "cycle the sort order" },
    Binding { context: C::Files, keys: &[ch('g')], label: "g", action: A::GoTo, help: "go to a path" },
    // ---- server tree --------------------------------------------------------------------------
    Binding { context: C::Tree, keys: &[plain(KeyCode::Up), ch('k')], label: "Up Down", action: A::Up, help: "move the cursor" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Down), ch('j')], label: "", action: A::Down, help: "" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::PageUp)], label: "PgUp PgDn", action: A::PageUp, help: "page up / down" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::PageDown)], label: "", action: A::PageDown, help: "" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Home)], label: "Home End", action: A::Home, help: "first / last row" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::End)], label: "", action: A::End, help: "" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Right), ch('l')], label: "Right Left", action: A::Expand, help: "expand / collapse a folder" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Left), ch('h')], label: "", action: A::Collapse, help: "" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Enter)], label: "Enter", action: A::Open, help: "connect to a site, or open / close a folder" },
    Binding { context: C::Tree, keys: &[ch('x')], label: "x", action: A::Disconnect, help: "disconnect" },
    Binding { context: C::Tree, keys: &[ch('n')], label: "n", action: A::NewSite, help: "new site" },
    Binding { context: C::Tree, keys: &[ch('N')], label: "N", action: A::NewFolder, help: "new folder" },
    Binding { context: C::Tree, keys: &[ch('e')], label: "e", action: A::EditSite, help: "edit the site" },
    Binding { context: C::Tree, keys: &[ch('r')], label: "r", action: A::RenameNode, help: "rename" },
    Binding { context: C::Tree, keys: &[ch('D')], label: "D", action: A::DuplicateSite, help: "duplicate the site" },
    Binding { context: C::Tree, keys: &[ch('m')], label: "m", action: A::MoveNode, help: "move to a folder" },
    Binding { context: C::Tree, keys: &[plain(KeyCode::Delete)], label: "Del", action: A::DeleteNode, help: "delete the site or folder" },
    Binding { context: C::Tree, keys: &[ch('i')], label: "i", action: A::ImportFileZilla, help: "import a FileZilla sitemanager.xml" },
    // ---- queue, completed and failed lists ----------------------------------------------------
    Binding { context: C::Queue, keys: &[plain(KeyCode::Up), ch('k')], label: "Up Down", action: A::Up, help: "move the cursor" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::Down), ch('j')], label: "", action: A::Down, help: "" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::PageUp)], label: "PgUp PgDn", action: A::PageUp, help: "page up / down" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::PageDown)], label: "", action: A::PageDown, help: "" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::Home)], label: "Home End", action: A::Home, help: "first / last row" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::End)], label: "", action: A::End, help: "" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::Right), ch('l')], label: "Right Left", action: A::TabNext, help: "next / previous tab" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::Left), ch('h')], label: "", action: A::TabPrev, help: "" },
    Binding { context: C::Queue, keys: &[ch('p')], label: "p", action: A::QueuePause, help: "pause / resume the queue" },
    Binding { context: C::Queue, keys: &[plain(KeyCode::Delete), ch('d')], label: "Del d", action: A::QueueRemove, help: "remove the item (cancels it when running)" },
    Binding { context: C::Queue, keys: &[ch('r')], label: "r", action: A::QueueRetry, help: "retry the failed item" },
    Binding { context: C::Queue, keys: &[ch('R')], label: "R", action: A::QueueRetryAll, help: "retry every failed item" },
    Binding { context: C::Queue, keys: &[ch('C')], label: "C", action: A::QueueClear, help: "clear the completed list" },
    // ---- log ----------------------------------------------------------------------------------
    Binding { context: C::Log, keys: &[plain(KeyCode::Up), ch('k')], label: "Up Down", action: A::Up, help: "scroll the log" },
    Binding { context: C::Log, keys: &[plain(KeyCode::Down), ch('j')], label: "", action: A::Down, help: "" },
    Binding { context: C::Log, keys: &[plain(KeyCode::PageUp)], label: "PgUp PgDn", action: A::PageUp, help: "scroll a page" },
    Binding { context: C::Log, keys: &[plain(KeyCode::PageDown)], label: "", action: A::PageDown, help: "" },
    Binding { context: C::Log, keys: &[plain(KeyCode::Home)], label: "Home", action: A::Home, help: "oldest line" },
    Binding { context: C::Log, keys: &[plain(KeyCode::End), ch('f')], label: "End f", action: A::LogFollow, help: "follow the newest line" },
    Binding { context: C::Log, keys: &[plain(KeyCode::Right), ch('l')], label: "Right Left", action: A::TabNext, help: "next / previous tab" },
    Binding { context: C::Log, keys: &[plain(KeyCode::Left), ch('h')], label: "", action: A::TabPrev, help: "" },
    // ---- terminal tab (every other key goes to the shell) -------------------------------------
    Binding { context: C::Terminal, keys: &[ctrl('\\'), f(12)], label: "Ctrl-\\ F12", action: A::TerminalEscape, help: "leave the terminal (the shell keeps running)" },
    Binding { context: C::Terminal, keys: &[shifted(KeyCode::PageUp)], label: "Shift-PgUp", action: A::TerminalScrollUp, help: "scroll back through the output" },
    Binding { context: C::Terminal, keys: &[shifted(KeyCode::PageDown)], label: "Shift-PgDn", action: A::TerminalScrollDown, help: "scroll forward" },
];

fn normalize(key: KeyEvent) -> (KeyCode, KeyModifiers) {
    // terminals disagree on whether `N` arrives with SHIFT: letters are matched by their case
    let mut mods = key.modifiers;
    // and BackTab *is* Shift-Tab: crossterm reports it with SHIFT, other terminals without
    if matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab) {
        mods.remove(KeyModifiers::SHIFT);
    }
    (key.code, mods)
}

fn matches(spec: &KeySpec, code: KeyCode, mods: KeyModifiers) -> bool {
    let mut spec_mods = spec.mods;
    if matches!(spec.code, KeyCode::Char(_) | KeyCode::BackTab) {
        spec_mods.remove(KeyModifiers::SHIFT);
    }
    spec.code == code && spec_mods == mods
}

/// Like [`lookup`] but without the global fallback: for the terminal tab, where keys such as
/// `Tab` and `q` belong to the shell.
pub fn lookup_exact(context: Context, key: KeyEvent) -> Option<Action> {
    let (code, mods) = normalize(key);
    BINDINGS
        .iter()
        .filter(|b| b.context == context)
        .find(|b| b.keys.iter().any(|k| matches(k, code, mods)))
        .map(|b| b.action)
}

/// The action bound to `key` in `context`, falling back to the global bindings.
pub fn lookup(context: Context, key: KeyEvent) -> Option<Action> {
    let (code, mods) = normalize(key);
    let find = |wanted: Context| {
        BINDINGS
            .iter()
            .filter(|b| b.context == wanted)
            .find(|b| b.keys.iter().any(|k| matches(k, code, mods)))
            .map(|b| b.action)
    };
    find(context).or_else(|| find(Context::Global))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_tab_focuses_the_previous_area_with_or_without_shift() {
        // crossterm reports Shift-Tab as BackTab + SHIFT
        for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let event = KeyEvent::new(KeyCode::BackTab, mods);
            assert_eq!(lookup(Context::Global, event), Some(Action::FocusPrev));
            assert_eq!(lookup(Context::Files, event), Some(Action::FocusPrev));
        }
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(lookup(Context::Global, shift_tab), Some(Action::FocusPrev));
    }

    #[test]
    fn the_terminal_tab_does_not_bind_back_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(lookup_exact(Context::Terminal, event), None);
    }
}
