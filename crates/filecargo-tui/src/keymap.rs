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

pub static BINDINGS: &[Binding] = &[
    // ---- global ----------------------------------------------------------------------------
    Binding {
        context: C::Global,
        keys: &[plain(KeyCode::Tab)],
        label: "Tab",
        action: A::FocusNext,
        help: "focus the next area",
    },
    Binding {
        context: C::Global,
        keys: &[plain(KeyCode::BackTab), shifted(KeyCode::Tab)],
        label: "Shift-Tab",
        action: A::FocusPrev,
        help: "focus the previous area",
    },
    Binding {
        context: C::Global,
        keys: &[f(1), ch('?')],
        label: "F1 ?",
        action: A::Help,
        help: "this help",
    },
    Binding {
        context: C::Global,
        keys: &[ctrl('r')],
        label: "Ctrl-r",
        action: A::Refresh,
        help: "refresh the focused pane",
    },
    Binding {
        context: C::Global,
        keys: &[alt('1')],
        label: "Alt-1",
        action: A::BottomTab(0),
        help: "bottom tab: Queue",
    },
    Binding {
        context: C::Global,
        keys: &[alt('2')],
        label: "Alt-2",
        action: A::BottomTab(1),
        help: "bottom tab: Completed",
    },
    Binding {
        context: C::Global,
        keys: &[alt('3')],
        label: "Alt-3",
        action: A::BottomTab(2),
        help: "bottom tab: Failed",
    },
    Binding {
        context: C::Global,
        keys: &[alt('4')],
        label: "Alt-4",
        action: A::BottomTab(3),
        help: "bottom tab: Log",
    },
    Binding {
        context: C::Global,
        keys: &[alt('5')],
        label: "Alt-5",
        action: A::BottomTab(4),
        help: "bottom tab: Terminal",
    },
    Binding {
        context: C::Global,
        keys: &[f(9)],
        label: "F9",
        action: A::ToggleTree,
        help: "show / hide the server tree",
    },
    Binding {
        context: C::Global,
        keys: &[f(10)],
        label: "F10",
        action: A::MaximizeBottom,
        help: "maximize / restore the bottom panel",
    },
    Binding {
        context: C::Global,
        keys: &[ch('q'), ctrl('q')],
        label: "q Ctrl-q",
        action: A::Quit,
        help: "quit",
    },
    // ---- file panes ---------------------------------------------------------------------------
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::Up), ch('k')],
        label: "Up Down",
        action: A::Up,
        help: "move the cursor",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::Down), ch('j')],
        label: "",
        action: A::Down,
        help: "",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::PageUp)],
        label: "PgUp PgDn",
        action: A::PageUp,
        help: "page up / down",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::PageDown)],
        label: "",
        action: A::PageDown,
        help: "",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::Home)],
        label: "Home End",
        action: A::Home,
        help: "first / last entry",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::End)],
        label: "",
        action: A::End,
        help: "",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::Enter)],
        label: "Enter",
        action: A::Open,
        help: "open a folder, or transfer a file to the other side",
    },
    Binding {
        context: C::Files,
        keys: &[plain(KeyCode::Backspace)],
        label: "Backspace",
        action: A::Parent,
        help: "parent folder",
    },
    Binding {
        context: C::Files,
        keys: &[ch(' '), plain(KeyCode::Insert)],
        label: "Space Ins",
        action: A::ToggleSelect,
        help: "select / unselect, move down",
    },
    Binding {
        context: C::Files,
        keys: &[ch('*')],
        label: "*",
        action: A::InvertSelection,
        help: "invert the selection",
    },
    Binding {
        context: C::Files,
        keys: &[ctrl('a')],
        label: "Ctrl-a",
        action: A::SelectAll,
        help: "select all",
    },
    Binding {
        context: C::Files,
        keys: &[f(5), ch('t')],
        label: "F5 t",
        action: A::Transfer,
        help: "transfer the selection (or the cursor row) to the other side",
    },
    Binding {
        context: C::Files,
        keys: &[f(7)],
        label: "F7",
        action: A::MakeDir,
        help: "new remote folder",
    },
    Binding {
        context: C::Files,
        keys: &[f(2)],
        label: "F2",
        action: A::Rename,
        help: "rename (remote)",
    },
    Binding {
        context: C::Files,
        keys: &[f(8), plain(KeyCode::Delete)],
        label: "F8 Del",
        action: A::Delete,
        help: "delete (remote)",
    },
    Binding {
        context: C::Files,
        keys: &[ch('c')],
        label: "c",
        action: A::Chmod,
        help: "change permissions (remote)",
    },
    Binding {
        context: C::Files,
        keys: &[ch('.')],
        label: ".",
        action: A::ToggleHidden,
        help: "show / hide dotfiles",
    },
    Binding {
        context: C::Files,
        keys: &[ch('s')],
        label: "s",
        action: A::CycleSort,
        help: "cycle the sort order",
    },
    Binding {
        context: C::Files,
        keys: &[ch('g')],
        label: "g",
        action: A::GoTo,
        help: "go to a path",
    },
];

fn normalize(key: KeyEvent) -> (KeyCode, KeyModifiers) {
    // terminals disagree on whether `N` arrives with SHIFT: letters are matched by their case
    let mut mods = key.modifiers;
    if matches!(key.code, KeyCode::Char(_)) {
        mods.remove(KeyModifiers::SHIFT);
    }
    (key.code, mods)
}

fn matches(spec: &KeySpec, code: KeyCode, mods: KeyModifiers) -> bool {
    let mut spec_mods = spec.mods;
    if matches!(spec.code, KeyCode::Char(_)) {
        spec_mods.remove(KeyModifiers::SHIFT);
    }
    spec.code == code && spec_mods == mods
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
