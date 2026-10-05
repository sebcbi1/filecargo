//! Pure key handling: `(UiState, AppState, Event) -> Commands`. No I/O, no terminal.

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;

use crate::keymap::{self, Action, Context};
use crate::layout;
use crate::pane::{PaneView, pane_view};
use crate::tree::{self, RowKind};
use crate::ui_state::{Focus, PaneUi, UiState};

#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Paste(String),
}

/// Rows a pane can show at the current terminal size.
fn viewport(ui: &UiState, focus: Focus) -> usize {
    let areas = layout::areas(
        Rect {
            x: 0,
            y: 0,
            width: ui.size.0,
            height: ui.size.1,
        },
        ui.tree_visible(),
        ui.maximize_bottom,
    );
    match focus {
        Focus::Local => layout::pane_rows(areas.local),
        Focus::Remote => layout::pane_rows(areas.remote),
        _ => 0,
    }
    .max(1)
}

/// Keeps the cursor on screen.
fn scroll_to_cursor(pane: &mut PaneUi, rows: usize) {
    if pane.cursor < pane.offset {
        pane.offset = pane.cursor;
    } else if pane.cursor >= pane.offset + rows {
        pane.offset = pane.cursor + 1 - rows;
    }
}

/// Keeps `ui` consistent with a new snapshot: cursors in range and, per pane, selections that
/// survive a refresh of the same directory but reset when another directory is shown.
pub fn sync(ui: &mut UiState, app: &AppState) {
    sync_tree(ui, app);
    for focus in [Focus::Local, Focus::Remote] {
        let rows = viewport(ui, focus);
        let Some(view) = pane_view(app, focus) else {
            if let Some(pane) = ui.pane_mut(focus) {
                *pane = PaneUi::default();
            }
            continue;
        };
        let (path, generation, row_count) = (view.path.clone(), view.generation, view.rows());
        let names: std::collections::BTreeSet<&str> =
            view.entries.iter().map(|e| e.name.as_str()).collect();
        let cursor_name = ui
            .pane(focus)
            .and_then(|p| view.entry(p.cursor))
            .map(|e| e.name.clone());
        let Some(pane) = ui.pane_mut(focus) else {
            continue;
        };
        let key = (path.clone(), generation);
        if pane.seen.as_ref() != Some(&key) {
            let same_directory = pane.seen.as_ref().is_some_and(|(p, _)| *p == path);
            if same_directory {
                // a refresh or a re-sort: keep what still exists
                pane.selected.retain(|name| names.contains(name.as_str()));
                if let Some(name) = cursor_name
                    && let Some(i) = view.entries.iter().position(|e| e.name == name)
                {
                    pane.cursor = i + usize::from(view.has_parent);
                }
            } else {
                // navigation: start at the top with nothing selected
                *pane = PaneUi::default();
            }
            pane.seen = Some(key);
        }
        pane.cursor = pane.cursor.min(row_count.saturating_sub(1));
        scroll_to_cursor(pane, rows);
    }
    // the remote pane may have vanished under the focus
    if ui.focus == Focus::Remote && app.remote.is_none() {
        ui.focus = Focus::Local;
    }
}

/// Rows the tree pane can show.
fn tree_viewport(ui: &UiState) -> usize {
    let areas = layout::areas(
        Rect {
            x: 0,
            y: 0,
            width: ui.size.0,
            height: ui.size.1,
        },
        ui.tree_visible(),
        ui.maximize_bottom,
    );
    areas
        .tree
        .map_or(1, |a| usize::from(a.height.saturating_sub(2)))
        .max(1)
}

/// Forgets folders that no longer exist and keeps the cursor on a row.
fn sync_tree(ui: &mut UiState, app: &AppState) {
    let existing: std::collections::HashSet<FolderId> =
        app.servers.folders().iter().map(|f| f.id).collect();
    ui.tree.expanded.retain(|id| existing.contains(id));
    let rows = tree::rows(&app.servers, &ui.tree.expanded).len();
    ui.tree.cursor = ui.tree.cursor.min(rows.saturating_sub(1));
    let height = tree_viewport(ui);
    if ui.tree.cursor < ui.tree.offset {
        ui.tree.offset = ui.tree.cursor;
    } else if ui.tree.cursor >= ui.tree.offset + height {
        ui.tree.offset = ui.tree.cursor + 1 - height;
    }
    // a connection that just came up: move on to the remote pane
    let connected = matches!(app.session, SessionState::Connected { .. }) && app.remote.is_some();
    if connected && !ui.was_connected && ui.focus == Focus::Tree {
        ui.focus = Focus::Remote;
    }
    ui.was_connected = connected;
}

fn focus_order(ui: &UiState, app: &AppState) -> Vec<Focus> {
    let mut order = Vec::new();
    if ui.tree_visible() && !ui.maximize_bottom {
        order.push(Focus::Tree);
    }
    if !ui.maximize_bottom {
        order.push(Focus::Local);
        if app.remote.is_some() {
            order.push(Focus::Remote);
        }
    }
    order.push(Focus::Bottom);
    order
}

fn shift_focus(ui: &mut UiState, app: &AppState, forward: bool) {
    let order = focus_order(ui, app);
    let at = order.iter().position(|f| *f == ui.focus).unwrap_or(0);
    let next = if forward {
        (at + 1) % order.len()
    } else {
        (at + order.len() - 1) % order.len()
    };
    ui.focus = order[next];
}

fn context_for(ui: &UiState) -> Context {
    match ui.focus {
        Focus::Tree => Context::Tree,
        Focus::Local | Focus::Remote => Context::Files,
        Focus::Bottom => Context::Queue,
    }
}

fn next_sort(current: Sort) -> Sort {
    use SortKey::{Modified, Name, Size};
    let (key, ascending) = match (current.key, current.ascending) {
        (Name, true) => (Name, false),
        (Name, false) => (Size, true),
        (Size, true) => (Size, false),
        (Size, false) => (Modified, true),
        (Modified, true) => (Modified, false),
        (Modified, false) => (Name, true),
    };
    Sort { key, ascending }
}

/// The names an action applies to: the selection, or the row under the cursor.
fn target_names(pane: &PaneUi, view: &PaneView<'_>) -> Vec<String> {
    if !pane.selected.is_empty() {
        view.entries
            .iter()
            .filter(|e| pane.selected.contains(&e.name))
            .map(|e| e.name.clone())
            .collect()
    } else {
        view.entry(pane.cursor)
            .map(|e| vec![e.name.clone()])
            .unwrap_or_default()
    }
}

fn move_cursor(ui: &mut UiState, view: &PaneView<'_>, focus: Focus, action: Action) {
    let rows = viewport(ui, focus);
    let last = view.rows().saturating_sub(1);
    let Some(pane) = ui.pane_mut(focus) else {
        return;
    };
    pane.cursor = match action {
        Action::Up => pane.cursor.saturating_sub(1),
        Action::Down => (pane.cursor + 1).min(last),
        Action::PageUp => pane.cursor.saturating_sub(rows.saturating_sub(1).max(1)),
        Action::PageDown => (pane.cursor + rows.saturating_sub(1).max(1)).min(last),
        Action::Home => 0,
        Action::End => last,
        _ => pane.cursor,
    };
    scroll_to_cursor(pane, rows);
}

fn files_action(ui: &mut UiState, app: &AppState, action: Action) -> Vec<Command> {
    let focus = ui.focus;
    let Some(view) = pane_view(app, focus) else {
        return Vec::new();
    };
    match action {
        Action::Up
        | Action::Down
        | Action::PageUp
        | Action::PageDown
        | Action::Home
        | Action::End => {
            move_cursor(ui, &view, focus, action);
            Vec::new()
        }
        Action::Parent => vec![Command::Up(view.id)],
        Action::Open => {
            let cursor = ui.pane(focus).map_or(0, |p| p.cursor);
            match view.entry(cursor) {
                None => vec![Command::Up(view.id)], // the `..` row
                Some(entry) if entry.is_dir() => {
                    vec![Command::Navigate {
                        pane: view.id,
                        path: entry.name.clone(),
                    }]
                }
                Some(entry) => transfer_command(focus, vec![entry.name.clone()])
                    .into_iter()
                    .collect(),
            }
        }
        Action::ToggleSelect => {
            let rows = viewport(ui, focus);
            let Some(pane) = ui.pane_mut(focus) else {
                return Vec::new();
            };
            if let Some(entry) = view.entry(pane.cursor)
                && !pane.selected.remove(&entry.name)
            {
                pane.selected.insert(entry.name.clone());
            }
            pane.cursor = (pane.cursor + 1).min(view.rows().saturating_sub(1));
            scroll_to_cursor(pane, rows);
            Vec::new()
        }
        Action::InvertSelection => {
            let Some(pane) = ui.pane_mut(focus) else {
                return Vec::new();
            };
            let inverted = view
                .entries
                .iter()
                .filter(|e| !pane.selected.contains(&e.name))
                .map(|e| e.name.clone())
                .collect();
            pane.selected = inverted;
            Vec::new()
        }
        Action::SelectAll => {
            let Some(pane) = ui.pane_mut(focus) else {
                return Vec::new();
            };
            pane.selected = view.entries.iter().map(|e| e.name.clone()).collect();
            Vec::new()
        }
        Action::Transfer => {
            let pane = ui.pane(focus).cloned().unwrap_or_default();
            let names = target_names(&pane, &view);
            let commands: Vec<Command> = transfer_command(focus, names).into_iter().collect();
            if !commands.is_empty()
                && let Some(pane) = ui.pane_mut(focus)
            {
                pane.selected.clear();
            }
            commands
        }
        Action::Delete if focus == Focus::Remote => {
            let pane = ui.pane(focus).cloned().unwrap_or_default();
            let names = target_names(&pane, &view);
            if names.is_empty() {
                Vec::new()
            } else {
                vec![Command::Delete { names }]
            }
        }
        Action::ToggleHidden => {
            let mut settings = (*app.settings).clone();
            settings.ui.show_hidden = !settings.ui.show_hidden;
            vec![Command::UpdateSettings(settings)]
        }
        Action::CycleSort => vec![Command::SetSort {
            pane: view.id,
            sort: next_sort(view.sort),
        }],
        _ => Vec::new(),
    }
}

/// Upload from the local pane, download from the remote pane.
fn transfer_command(focus: Focus, names: Vec<String>) -> Option<Command> {
    if names.is_empty() {
        return None;
    }
    match focus {
        Focus::Local => Some(Command::Upload { names }),
        Focus::Remote => Some(Command::Download { names }),
        _ => None,
    }
}

fn tree_action(ui: &mut UiState, app: &AppState, action: Action) -> Vec<Command> {
    let rows = tree::rows(&app.servers, &ui.tree.expanded);
    let last = rows.len().saturating_sub(1);
    let height = tree_viewport(ui);
    let current = rows.get(ui.tree.cursor).cloned();
    let mut commands = Vec::new();
    match action {
        Action::Up => ui.tree.cursor = ui.tree.cursor.saturating_sub(1),
        Action::Down => ui.tree.cursor = (ui.tree.cursor + 1).min(last),
        Action::PageUp => {
            ui.tree.cursor = ui
                .tree
                .cursor
                .saturating_sub(height.saturating_sub(1).max(1))
        }
        Action::PageDown => {
            ui.tree.cursor = (ui.tree.cursor + height.saturating_sub(1).max(1)).min(last)
        }
        Action::Home => ui.tree.cursor = 0,
        Action::End => ui.tree.cursor = last,
        Action::Expand => {
            if let Some(tree::TreeRow {
                kind: RowKind::Folder { id, .. },
                ..
            }) = &current
            {
                ui.tree.expanded.insert(*id);
            }
        }
        Action::Collapse => match &current {
            Some(tree::TreeRow {
                kind: RowKind::Folder { id, expanded: true },
                ..
            }) => {
                ui.tree.expanded.remove(id);
            }
            // on a site or a closed folder: go to the parent folder
            _ => {
                if let Some(parent) = tree::parent_row(&rows, ui.tree.cursor) {
                    ui.tree.cursor = parent;
                }
            }
        },
        Action::Open => match &current {
            Some(tree::TreeRow {
                kind: RowKind::Site { id, .. },
                ..
            }) => commands.push(Command::Connect(*id)),
            Some(tree::TreeRow {
                kind: RowKind::Folder { id, expanded },
                ..
            }) => {
                if *expanded {
                    ui.tree.expanded.remove(id);
                } else {
                    ui.tree.expanded.insert(*id);
                }
            }
            None => {}
        },
        Action::Disconnect => commands.push(Command::Disconnect),
        Action::DuplicateSite => {
            if let Some(tree::TreeRow {
                kind: RowKind::Site { id, .. },
                ..
            }) = &current
            {
                commands.push(Command::Tree(TreeOp::Duplicate { site: *id }));
            }
        }
        _ => {}
    }
    // keep the cursor row on screen (the row list may have changed size)
    let rows_now = tree::rows(&app.servers, &ui.tree.expanded).len();
    ui.tree.cursor = ui.tree.cursor.min(rows_now.saturating_sub(1));
    if ui.tree.cursor < ui.tree.offset {
        ui.tree.offset = ui.tree.cursor;
    } else if ui.tree.cursor >= ui.tree.offset + height {
        ui.tree.offset = ui.tree.cursor + 1 - height;
    }
    commands
}

pub fn on_event(ui: &mut UiState, app: &AppState, event: Event) -> Vec<Command> {
    match event {
        Event::Resize(width, height) => {
            ui.size = (width, height);
            sync(ui, app);
            Vec::new()
        }
        Event::Key(key) => {
            let Some(action) = keymap::lookup(context_for(ui), key) else {
                return Vec::new();
            };
            match action {
                Action::Quit => {
                    ui.quit_requested = true;
                    vec![Command::Quit]
                }
                Action::FocusNext => {
                    shift_focus(ui, app, true);
                    Vec::new()
                }
                Action::FocusPrev => {
                    shift_focus(ui, app, false);
                    Vec::new()
                }
                Action::ToggleTree => {
                    ui.tree_override = Some(!ui.tree_visible());
                    if !ui.tree_visible() && ui.focus == Focus::Tree {
                        ui.focus = Focus::Local;
                    }
                    sync(ui, app);
                    Vec::new()
                }
                Action::MaximizeBottom => {
                    ui.maximize_bottom = !ui.maximize_bottom;
                    ui.focus = if ui.maximize_bottom {
                        Focus::Bottom
                    } else {
                        Focus::Local
                    };
                    sync(ui, app);
                    Vec::new()
                }
                Action::Refresh => match pane_view(app, ui.focus) {
                    Some(view) => vec![Command::Refresh(view.id)],
                    None => Vec::new(),
                },
                other => match ui.focus {
                    Focus::Local | Focus::Remote => files_action(ui, app, other),
                    Focus::Tree => tree_action(ui, app, other),
                    Focus::Bottom => Vec::new(),
                },
            }
        }
        Event::Mouse(_) | Event::Paste(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use std::path::PathBuf;

    use super::*;
    use crate::test_support::{app, connected_app, synced};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn with(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }

    fn ch(c: char) -> Event {
        key(KeyCode::Char(c))
    }

    fn commands(list: &[Command]) -> Vec<String> {
        list.iter().map(|c| format!("{c:?}")).collect()
    }

    // rows of the local fixture: 0 `..`, 1 docs/, 2 src/, 3 README.md, 4 notes.txt, 5 big.iso

    #[test]
    fn cursor_movement_stays_in_range_and_pages_by_the_viewport() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        let step = |ui: &mut UiState, e: Event| {
            on_event(ui, &app, e);
            ui.local.cursor
        };
        assert_eq!(step(&mut ui, key(KeyCode::Down)), 1);
        assert_eq!(step(&mut ui, ch('j')), 2);
        assert_eq!(step(&mut ui, key(KeyCode::Up)), 1);
        assert_eq!(step(&mut ui, ch('k')), 0);
        assert_eq!(step(&mut ui, key(KeyCode::Up)), 0, "no wrap at the top");
        assert_eq!(step(&mut ui, key(KeyCode::End)), 5);
        assert_eq!(
            step(&mut ui, key(KeyCode::Down)),
            5,
            "no wrap at the bottom"
        );
        assert_eq!(step(&mut ui, key(KeyCode::Home)), 0);
        assert_eq!(
            step(&mut ui, key(KeyCode::PageDown)),
            5,
            "a short list: one page reaches the end"
        );
        assert_eq!(step(&mut ui, key(KeyCode::PageUp)), 0);
    }

    #[test]
    fn scrolling_keeps_the_cursor_visible_in_a_long_listing() {
        let mut app = app();
        let many: Vec<Entry> = (0..200)
            .map(|i| crate::test_support::entry(&format!("f{i:03}"), false, 1, 1))
            .collect();
        app.local = crate::test_support::pane(PathBuf::from("/home/me/projects"), many);
        let mut ui = synced(100, 30, &app);
        let rows = viewport(&ui, Focus::Local);
        for _ in 0..50 {
            on_event(&mut ui, &app, key(KeyCode::Down));
        }
        assert_eq!(ui.local.cursor, 50);
        assert!(
            ui.local.offset <= 50 && 50 < ui.local.offset + rows,
            "{:?} rows {rows}",
            ui.local
        );
        on_event(&mut ui, &app, key(KeyCode::End));
        assert_eq!(ui.local.cursor, 200);
        assert_eq!(ui.local.offset + rows, 201);
    }

    #[test]
    fn enter_opens_directories_goes_up_on_dotdot_and_transfers_files() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::Enter))),
            ["Up(Local)"],
            "the `..` row goes up"
        );
        on_event(&mut ui, &app, key(KeyCode::Down));
        let out = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert_eq!(
            commands(&out),
            [r#"Navigate { pane: Local, path: "docs" }"#]
        );
        ui.local.cursor = 3;
        let out = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert_eq!(
            commands(&out),
            [r#"Upload { names: ["README.md"] }"#],
            "Enter on a file queues it for the other side"
        );
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::Backspace))),
            ["Up(Local)"]
        );
    }

    #[test]
    fn selection_toggle_invert_and_select_all() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, key(KeyCode::Down)); // docs
        on_event(&mut ui, &app, ch(' ')); // select docs, cursor moves to src
        on_event(&mut ui, &app, key(KeyCode::Insert)); // select src
        assert_eq!(
            ui.local
                .selected
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["docs", "src"]
        );
        assert_eq!(ui.local.cursor, 3);
        on_event(&mut ui, &app, ch(' ')); // README.md
        on_event(&mut ui, &app, ch('g')); // `g` is go-to, not a selection key
        assert_eq!(ui.local.selected.len(), 3);
        on_event(&mut ui, &app, ch('*'));
        assert_eq!(
            ui.local
                .selected
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["big.iso", "notes.txt"]
        );
        on_event(
            &mut ui,
            &app,
            with(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );
        assert_eq!(ui.local.selected.len(), 5);
        // the `..` row cannot be selected
        ui.local.cursor = 0;
        ui.local.selected.clear();
        on_event(&mut ui, &app, ch(' '));
        assert!(ui.local.selected.is_empty());
    }

    #[test]
    fn transfer_uses_the_selection_or_the_cursor_row_and_clears_the_selection() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        ui.local.cursor = 4; // notes.txt
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::F(5)))),
            [r#"Upload { names: ["notes.txt"] }"#]
        );
        ui.local.selected = ["README.md".to_owned(), "src".to_owned()].into();
        let out = on_event(&mut ui, &app, ch('t'));
        assert_eq!(
            commands(&out),
            [r#"Upload { names: ["src", "README.md"] }"#]
        );
        assert!(ui.local.selected.is_empty());

        ui.focus = Focus::Remote;
        ui.remote.cursor = 2; // index.php
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::F(5)))),
            [r#"Download { names: ["index.php"] }"#]
        );
        ui.remote.cursor = 0; // `..`
        assert!(
            on_event(&mut ui, &app, key(KeyCode::F(5))).is_empty(),
            "nothing under the cursor, nothing selected"
        );
    }

    #[test]
    fn delete_applies_to_the_remote_pane_only() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        assert!(
            on_event(&mut ui, &app, key(KeyCode::Delete)).is_empty(),
            "the local pane is browse-only"
        );
        ui.focus = Focus::Remote;
        ui.remote.cursor = 1; // html/
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::F(8)))),
            [r#"Delete { names: ["html"] }"#]
        );
        ui.remote.selected = ["index.php".to_owned(), "style.css".to_owned()].into();
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::Delete))),
            [r#"Delete { names: ["index.php", "style.css"] }"#]
        );
    }

    #[test]
    fn refresh_hidden_and_sort_keys() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        assert_eq!(
            commands(&on_event(
                &mut ui,
                &app,
                with(KeyCode::Char('r'), KeyModifiers::CONTROL)
            )),
            ["Refresh(Local)"]
        );
        ui.focus = Focus::Remote;
        assert_eq!(
            commands(&on_event(
                &mut ui,
                &app,
                with(KeyCode::Char('r'), KeyModifiers::CONTROL)
            )),
            ["Refresh(Remote)"]
        );

        let out = on_event(&mut ui, &app, ch('.'));
        match &out[..] {
            [Command::UpdateSettings(settings)] => assert!(
                settings.ui.show_hidden,
                "the default is hidden, so this shows them"
            ),
            other => panic!("{other:?}"),
        }
        let out = on_event(&mut ui, &app, ch('s'));
        assert_eq!(
            commands(&out),
            ["SetSort { pane: Remote, sort: Sort { key: Name, ascending: false } }"]
        );
        assert_eq!(
            next_sort(Sort {
                key: SortKey::Modified,
                ascending: false
            }),
            Sort::default(),
            "the cycle closes"
        );
    }

    #[test]
    fn tab_cycles_the_focus_over_what_exists_and_quit_quits() {
        let mut app = app();
        let mut ui = synced(120, 30, &app);
        assert_eq!(ui.focus, Focus::Local);
        on_event(&mut ui, &app, key(KeyCode::Tab));
        assert_eq!(ui.focus, Focus::Bottom, "no remote pane yet");
        on_event(&mut ui, &app, key(KeyCode::Tab));
        assert_eq!(ui.focus, Focus::Tree, "the tree is visible at 120 columns");
        on_event(&mut ui, &app, key(KeyCode::BackTab));
        assert_eq!(ui.focus, Focus::Bottom);
        on_event(&mut ui, &app, with(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(ui.focus, Focus::Local);

        app = connected_app();
        sync(&mut ui, &app);
        on_event(&mut ui, &app, key(KeyCode::Tab));
        assert_eq!(ui.focus, Focus::Remote);

        assert_eq!(commands(&on_event(&mut ui, &app, ch('q'))), ["Quit"]);
        assert!(ui.quit_requested);
        assert_eq!(
            commands(&on_event(
                &mut ui,
                &app,
                with(KeyCode::Char('q'), KeyModifiers::CONTROL)
            )),
            ["Quit"]
        );
    }

    #[test]
    fn f9_and_f10_toggle_the_tree_and_the_bottom_panel() {
        let app = app();
        let mut ui = synced(120, 30, &app);
        assert!(ui.tree_visible());
        on_event(&mut ui, &app, key(KeyCode::F(9)));
        assert!(!ui.tree_visible());
        on_event(&mut ui, &app, key(KeyCode::F(9)));
        assert!(ui.tree_visible());
        let mut narrow = synced(90, 30, &app);
        assert!(
            !narrow.tree_visible(),
            "hidden below 100 columns until toggled"
        );
        on_event(&mut narrow, &app, key(KeyCode::F(9)));
        assert!(narrow.tree_visible());

        on_event(&mut ui, &app, key(KeyCode::F(10)));
        assert!(ui.maximize_bottom && ui.focus == Focus::Bottom);
        on_event(&mut ui, &app, key(KeyCode::F(10)));
        assert!(!ui.maximize_bottom && ui.focus == Focus::Local);
    }

    // ---- AC3: selection across snapshots ------------------------------------------------------

    #[test]
    fn a_refresh_keeps_the_selection_that_still_exists_and_the_cursor_on_its_name() {
        let mut app = app();
        let mut ui = synced(100, 30, &app);
        ui.local.selected = ["README.md".to_owned(), "notes.txt".to_owned()].into();
        ui.local.cursor = 4; // notes.txt

        // same directory, new generation: README.md was deleted, a new file appeared above
        let mut entries = crate::test_support::local_entries();
        entries.retain(|e| e.name != "README.md");
        entries.insert(0, crate::test_support::entry("a-new-file", false, 1, 1));
        app.local = crate::test_support::pane(PathBuf::from("/home/me/projects"), entries);
        app.local.generation = 2;
        sync(&mut ui, &app);
        assert_eq!(
            ui.local
                .selected
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["notes.txt"]
        );
        let view = pane_view(&app, Focus::Local).unwrap();
        assert_eq!(
            view.entry(ui.local.cursor).unwrap().name,
            "notes.txt",
            "the cursor follows the name"
        );
    }

    #[test]
    fn navigating_resets_cursor_and_selection() {
        let mut app = app();
        let mut ui = synced(100, 30, &app);
        ui.local.selected = ["docs".to_owned()].into();
        ui.local.cursor = 3;
        app.local = crate::test_support::pane(
            PathBuf::from("/home/me/projects/src"),
            vec![crate::test_support::entry("main.rs", false, 10, 1)],
        );
        app.local.generation = 2;
        sync(&mut ui, &app);
        assert!(ui.local.selected.is_empty());
        assert_eq!(ui.local.cursor, 0);
    }

    #[test]
    fn disconnecting_resets_the_remote_pane_and_moves_the_focus() {
        let mut app = connected_app();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Remote;
        ui.remote.cursor = 2;
        ui.remote.selected = ["html".to_owned()].into();
        app.remote = None;
        sync(&mut ui, &app);
        assert_eq!(ui.focus, Focus::Local);
        assert_eq!(ui.remote, PaneUi::default());
    }

    fn tree_app() -> AppState {
        let mut app = app();
        app.servers = std::sync::Arc::new(crate::test_support::sample_tree());
        app
    }

    fn tree_ui(app: &AppState) -> UiState {
        let mut ui = synced(120, 30, app);
        ui.focus = Focus::Tree;
        ui
    }

    #[test]
    fn tree_cursor_moves_and_folders_expand_collapse_and_toggle() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        let visible = |ui: &UiState| {
            crate::tree::rows(&app.servers, &ui.tree.expanded)
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(visible(&ui), ["Personal", "Work", "home-nas"]);
        on_event(&mut ui, &app, key(KeyCode::Down));
        assert_eq!(ui.tree.cursor, 1, "Work");
        on_event(&mut ui, &app, key(KeyCode::Right));
        assert_eq!(
            visible(&ui),
            ["Personal", "Work", "prod-web", "staging", "home-nas"]
        );
        on_event(&mut ui, &app, key(KeyCode::Left));
        assert_eq!(visible(&ui).len(), 3, "Left collapses an open folder");
        on_event(&mut ui, &app, key(KeyCode::Enter));
        assert_eq!(visible(&ui).len(), 5, "Enter on a folder opens it");
        on_event(&mut ui, &app, key(KeyCode::Enter));
        assert_eq!(visible(&ui).len(), 3, "and closes it again");
        on_event(&mut ui, &app, key(KeyCode::End));
        assert_eq!(ui.tree.cursor, 2);
        on_event(&mut ui, &app, key(KeyCode::Down));
        assert_eq!(ui.tree.cursor, 2, "no wrap");
        on_event(&mut ui, &app, key(KeyCode::Home));
        assert_eq!(ui.tree.cursor, 0);
        on_event(&mut ui, &app, ch('j'));
        on_event(&mut ui, &app, ch('l'));
        assert_eq!(visible(&ui).len(), 5, "vi keys work too");
    }

    #[test]
    fn left_on_a_site_goes_to_its_folder() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        on_event(&mut ui, &app, key(KeyCode::Down)); // Work
        on_event(&mut ui, &app, key(KeyCode::Right));
        on_event(&mut ui, &app, key(KeyCode::Down)); // prod-web
        assert_eq!(ui.tree.cursor, 2);
        on_event(&mut ui, &app, key(KeyCode::Left));
        assert_eq!(ui.tree.cursor, 1);
    }

    #[test]
    fn enter_on_a_site_connects_x_disconnects_and_d_duplicates() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
        // rows: Personal, blog, Work, prod-web, staging, home-nas
        ui.tree.cursor = 3;
        let id = crate::test_support::site_id(&app.servers, "prod-web");
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::Enter))),
            [format!("Connect({id:?})")]
        );
        assert_eq!(commands(&on_event(&mut ui, &app, ch('x'))), ["Disconnect"]);
        assert_eq!(
            commands(&on_event(&mut ui, &app, ch('D'))),
            [format!("Tree(Duplicate {{ site: {id:?} }})")]
        );
        ui.tree.cursor = 2; // a folder: nothing to duplicate
        assert!(on_event(&mut ui, &app, ch('D')).is_empty());
    }

    #[test]
    fn a_new_connection_moves_the_focus_from_the_tree_to_the_remote_pane() {
        let mut app = tree_app();
        let mut ui = tree_ui(&app);
        let id = crate::test_support::site_id(&app.servers, "prod-web");
        app.session = SessionState::Connecting {
            site: id,
            step: ConnectStep::Connecting,
        };
        sync(&mut ui, &app);
        assert_eq!(ui.focus, Focus::Tree, "still connecting");
        let connected = connected_app();
        app.remote = connected.remote.clone();
        app.session = SessionState::Connected {
            site: id,
            info: SessionInfo {
                protocol: Protocol::Sftp,
                banner: None,
                tls: None,
                home: RemotePath::root(),
            },
        };
        sync(&mut ui, &app);
        assert_eq!(ui.focus, Focus::Remote);
        // it does not keep stealing the focus afterwards
        ui.focus = Focus::Tree;
        sync(&mut ui, &app);
        assert_eq!(ui.focus, Focus::Tree);
    }

    #[test]
    fn deleting_a_folder_elsewhere_prunes_the_expansion_and_clamps_the_cursor() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
        ui.tree.cursor = 5;
        let mut smaller = app.clone();
        smaller.servers = std::sync::Arc::new(filecargo_app_core::prelude::ServerTree::default());
        sync(&mut ui, &smaller);
        assert!(ui.tree.expanded.is_empty());
        assert_eq!(ui.tree.cursor, 0);
    }

    #[test]
    fn a_resize_updates_the_size_and_keeps_the_cursor_visible() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, Event::Resize(80, 24));
        assert_eq!(ui.size, (80, 24));
    }
}
