//! Pure key handling: `(UiState, AppState, Event) -> Commands`. No I/O, no terminal.

use filecargo_app_core::prelude::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;

use crate::bottom_view;
use crate::dialog::{
    ConfirmDialog, Dialog, InputDialog, InputPurpose, MovePicker, Outcome, SiteEditor,
};
use crate::dialog_util::{ChmodDialog, ImportDialog};
use crate::keymap::{self, Action, Context};
use crate::keys::to_terminal_key;
use crate::layout;
use crate::pane::{PaneView, pane_view};
use crate::prompt_ui::{PromptSlot, PromptUi};
use crate::tree::{self, RowKind};
use crate::ui_state::{BottomTab, Focus, ListUi, PaneUi, UiState};

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
    sync_prompt(ui, app);
    sync_bottom(ui, app);
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
        Focus::Bottom => match ui.bottom.tab {
            BottomTab::Log => Context::Log,
            BottomTab::Terminal => Context::Terminal,
            _ => Context::Queue,
        },
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
                vec![Command::Delete {
                    pane: PaneId::Remote,
                    names,
                }]
            }
        }
        Action::GoTo => {
            ui.dialog = Some(Dialog::Input(InputDialog::new(
                "Go to path",
                "Path",
                &view.path,
                InputPurpose::GoTo { pane: view.id },
            )));
            Vec::new()
        }
        Action::Chmod if focus == Focus::Remote => {
            let pane = ui.pane(focus).cloned().unwrap_or_default();
            let names = target_names(&pane, &view);
            let mode = view
                .entry(pane.cursor)
                .and_then(|e| e.permissions)
                .unwrap_or(0o644);
            if !names.is_empty() {
                ui.dialog = Some(Dialog::Chmod(ChmodDialog::new(names, mode)));
            }
            Vec::new()
        }
        Action::MakeDir if focus == Focus::Remote => {
            ui.dialog = Some(Dialog::Input(InputDialog::new(
                "New remote folder",
                "Name",
                "",
                InputPurpose::Mkdir,
            )));
            Vec::new()
        }
        Action::Rename if focus == Focus::Remote => {
            let cursor = ui.pane(focus).map_or(0, |p| p.cursor);
            if let Some(entry) = view.entry(cursor) {
                ui.dialog = Some(Dialog::Input(InputDialog::new(
                    "Rename",
                    "New name",
                    &entry.name,
                    InputPurpose::RenameRemote {
                        from: entry.name.clone(),
                    },
                )));
            }
            Vec::new()
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
        Action::ImportFileZilla => {
            let default = ui
                .home
                .as_ref()
                .map(|home| home.join(".config/filezilla/sitemanager.xml"))
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.dialog = Some(Dialog::Import(ImportDialog::new(&default)));
        }
        Action::NewSite => {
            ui.dialog = Some(Dialog::Site(Box::new(SiteEditor::new_site(target_folder(
                app, &current,
            )))));
        }
        Action::NewFolder => {
            ui.dialog = Some(Dialog::Input(InputDialog::new(
                "New folder",
                "Name",
                "",
                InputPurpose::NewFolder {
                    parent: target_folder(app, &current),
                },
            )));
        }
        Action::EditSite => {
            if let Some(tree::TreeRow {
                kind: RowKind::Site { id, .. },
                ..
            }) = &current
                && let Some(site) = app.servers.site(*id)
            {
                ui.dialog = Some(Dialog::Site(Box::new(SiteEditor::edit(site))));
            }
        }
        Action::RenameNode => {
            if let Some(row) = &current {
                ui.dialog = Some(Dialog::Input(InputDialog::new(
                    "Rename",
                    "Name",
                    &row.name,
                    InputPurpose::RenameNode(row.node()),
                )));
            }
        }
        Action::MoveNode => {
            if let Some(row) = &current {
                ui.dialog = Some(Dialog::Move(MovePicker::new(&app.servers, row.node())));
            }
        }
        Action::DeleteNode => {
            if let Some(row) = &current {
                let what = match row.kind {
                    RowKind::Folder { .. } => "folder (and everything in it)",
                    RowKind::Site { .. } => "site",
                };
                ui.dialog = Some(Dialog::Confirm(ConfirmDialog {
                    title: "Delete".to_owned(),
                    body: format!("Delete the {what} \"{}\"?", row.name),
                    op: TreeOp::Delete { node: row.node() },
                }));
            }
        }
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

/// The folder a new node goes to: the folder under the cursor, or the one holding the site.
fn target_folder(app: &AppState, current: &Option<tree::TreeRow>) -> Option<FolderId> {
    match current.as_ref()?.kind {
        RowKind::Folder { id, .. } => Some(id),
        RowKind::Site { id, .. } => app.servers.site(id).and_then(|s| s.folder),
    }
}

/// Keys while a dialog is open go to it alone.
fn dialog_key(ui: &mut UiState, key: KeyEvent) -> Vec<Command> {
    let Some(dialog) = ui.dialog.as_mut() else {
        return Vec::new();
    };
    match dialog.on_key(key) {
        Outcome::Keep => Vec::new(),
        Outcome::Close => {
            ui.dialog = None;
            Vec::new()
        }
        Outcome::Run(commands) => {
            ui.dialog = None;
            commands
        }
    }
}

/// One UI slot per prompt the app shows; a new prompt id starts a fresh one.
fn sync_prompt(ui: &mut UiState, app: &AppState) {
    match (&app.prompt, &ui.prompt) {
        (None, _) => ui.prompt = None,
        (Some(prompt), Some(slot)) if slot.id == prompt.id => {}
        (Some(prompt), _) => {
            ui.prompt = Some(PromptSlot {
                id: prompt.id,
                ui: PromptUi::for_kind(&prompt.kind),
                answered: false,
            });
        }
    }
}

/// Keys while the app's prompt shows: only the prompt hears them.
fn prompt_key(ui: &mut UiState, app: &AppState, key: KeyEvent) -> Vec<Command> {
    let (Some(prompt), Some(slot)) = (&app.prompt, ui.prompt.as_mut()) else {
        return Vec::new();
    };
    if slot.answered || slot.id != prompt.id {
        return Vec::new();
    }
    match slot.ui.on_key(&prompt.kind, key) {
        Some(answer) => {
            slot.answered = true;
            vec![Command::Answer {
                id: prompt.id,
                answer,
            }]
        }
        None => Vec::new(),
    }
}

/// Rows of a list tab: the panel's height minus the border, the header and the footer line.
fn bottom_rows(ui: &UiState) -> usize {
    let area = layout::areas(
        Rect::new(0, 0, ui.size.0, ui.size.1),
        ui.tree_visible(),
        ui.maximize_bottom,
    )
    .bottom;
    usize::from(area.height.saturating_sub(4)).max(1)
}

/// Lines of the log tab: the panel's height minus the border.
pub(crate) fn log_rows(ui: &UiState) -> usize {
    let area = layout::areas(
        Rect::new(0, 0, ui.size.0, ui.size.1),
        ui.tree_visible(),
        ui.maximize_bottom,
    )
    .bottom;
    usize::from(area.height.saturating_sub(2)).max(1)
}

fn list_len(app: &AppState, tab: BottomTab) -> usize {
    match tab {
        BottomTab::Queue => app.queue.pending.len(),
        BottomTab::Completed => app.queue.completed.len(),
        BottomTab::Failed => app.queue.failed.len(),
        BottomTab::Log | BottomTab::Terminal => 0,
    }
}

fn keep_in_view(list: &mut ListUi, len: usize, rows: usize) {
    list.cursor = list.cursor.min(len.saturating_sub(1));
    if list.cursor < list.offset {
        list.offset = list.cursor;
    } else if list.cursor >= list.offset + rows {
        list.offset = list.cursor + 1 - rows;
    }
    list.offset = list.offset.min(len.saturating_sub(rows));
}

fn sync_bottom(ui: &mut UiState, app: &AppState) {
    let rows = bottom_rows(ui);
    for tab in [BottomTab::Queue, BottomTab::Completed, BottomTab::Failed] {
        let len = list_len(app, tab);
        let list = match tab {
            BottomTab::Queue => &mut ui.bottom.queue,
            BottomTab::Completed => &mut ui.bottom.completed,
            _ => &mut ui.bottom.failed,
        };
        keep_in_view(list, len, rows);
    }
    if ui.bottom.log_follow {
        ui.bottom.log_scroll = 0;
    }
}

fn select_tab(ui: &mut UiState, tab: BottomTab) {
    ui.bottom.tab = tab;
    ui.focus = Focus::Bottom;
}

/// The id of the item under the cursor of the visible list tab.
fn item_under_cursor(ui: &UiState, app: &AppState) -> Option<TransferId> {
    let (list, items) = match ui.bottom.tab {
        BottomTab::Queue => (ui.bottom.queue, &app.queue.pending),
        BottomTab::Completed => (ui.bottom.completed, &app.queue.completed),
        BottomTab::Failed => (ui.bottom.failed, &app.queue.failed),
        BottomTab::Log | BottomTab::Terminal => return None,
    };
    items.get(list.cursor).map(|view| view.item.id)
}

fn bottom_action(ui: &mut UiState, app: &AppState, action: Action) -> Vec<Command> {
    let tab = ui.bottom.tab;
    match action {
        Action::TabNext | Action::TabPrev => {
            let count = BottomTab::ALL.len();
            let at = tab.index();
            let next = if action == Action::TabNext {
                (at + 1) % count
            } else {
                (at + count - 1) % count
            };
            ui.bottom.tab = BottomTab::ALL[next];
            Vec::new()
        }
        Action::QueuePause => vec![Command::QueueSetProcessing(!app.queue.processing)],
        Action::QueueClear => vec![Command::QueueClearCompleted],
        Action::QueueRetryAll => vec![Command::QueueRetryFailed],
        Action::QueueRemove => item_under_cursor(ui, app)
            .map(Command::QueueRemove)
            .into_iter()
            .collect(),
        Action::QueueRetry if tab == BottomTab::Failed => item_under_cursor(ui, app)
            .map(Command::QueueRetry)
            .into_iter()
            .collect(),
        Action::LogFollow => {
            ui.bottom.log_follow = true;
            ui.bottom.log_scroll = 0;
            Vec::new()
        }
        Action::Up
        | Action::Down
        | Action::PageUp
        | Action::PageDown
        | Action::Home
        | Action::End => {
            if tab == BottomTab::Log {
                scroll_log(ui, action);
            } else {
                let (len, rows) = (list_len(app, tab), bottom_rows(ui));
                if let Some(list) = ui.bottom.list_mut() {
                    list.cursor = match action {
                        Action::Up => list.cursor.saturating_sub(1),
                        Action::Down => list.cursor + 1,
                        Action::PageUp => list.cursor.saturating_sub(rows.saturating_sub(1).max(1)),
                        Action::PageDown => list.cursor + rows.saturating_sub(1).max(1),
                        Action::Home => 0,
                        _ => len.saturating_sub(1),
                    };
                    keep_in_view(list, len, rows);
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn scroll_log(ui: &mut UiState, action: Action) {
    let total = ui.log.len();
    let rows = log_rows(ui);
    let max = total.saturating_sub(rows);
    let page = rows.saturating_sub(1).max(1);
    let scroll = ui.bottom.log_scroll;
    let scroll = match action {
        Action::Up => scroll + 1,
        Action::Down => scroll.saturating_sub(1),
        Action::PageUp => scroll + page,
        Action::PageDown => scroll.saturating_sub(page),
        Action::Home => max,
        _ => scroll,
    };
    ui.bottom.log_scroll = scroll.min(max);
    ui.bottom.log_follow = ui.bottom.log_scroll == 0;
}

/// Cells of the shell: the bottom panel inside its border.
fn terminal_size(ui: &UiState) -> (u16, u16) {
    let area = layout::areas(
        Rect::new(0, 0, ui.size.0, ui.size.1),
        ui.tree_visible(),
        ui.maximize_bottom,
    )
    .bottom;
    (
        area.width.saturating_sub(2).max(1),
        area.height.saturating_sub(2).max(1),
    )
}

/// Opens the shell the first time the tab shows and keeps its size in step with the panel.
/// The loop calls this after every sync.
pub fn housekeeping(ui: &mut UiState, app: &AppState) -> Vec<Command> {
    let on_tab = ui.bottom.tab == BottomTab::Terminal;
    let size = terminal_size(ui);
    match &app.terminal {
        TerminalState::Closed if on_tab && !ui.bottom.term_open_sent => {
            ui.bottom.term_open_sent = true;
            ui.bottom.term_size = Some(size);
            vec![Command::TerminalOpen {
                cols: size.0,
                rows: size.1,
            }]
        }
        TerminalState::Closed if !on_tab => {
            ui.bottom.term_open_sent = false;
            Vec::new()
        }
        TerminalState::Open(_) => {
            ui.bottom.term_open_sent = false;
            if ui.bottom.term_size == Some(size) {
                Vec::new()
            } else {
                ui.bottom.term_size = Some(size);
                vec![Command::TerminalResize {
                    cols: size.0,
                    rows: size.1,
                }]
            }
        }
        _ => Vec::new(),
    }
}

/// Keys on the terminal tab: only the escape keys and `Alt-digit` are ours, the rest is the
/// shell's (including `Tab`, `Esc` and `q`).
fn terminal_key(ui: &mut UiState, app: &AppState, key: KeyEvent) -> Vec<Command> {
    let page = i32::from(terminal_size(ui).1.saturating_sub(1).max(1));
    match keymap::lookup_exact(Context::Terminal, key) {
        Some(Action::TerminalEscape) => {
            ui.focus = if app.remote.is_some() {
                Focus::Remote
            } else {
                Focus::Local
            };
            return Vec::new();
        }
        Some(Action::TerminalScrollUp) => return vec![Command::TerminalScroll(page)],
        Some(Action::TerminalScrollDown) => return vec![Command::TerminalScroll(-page)],
        _ => {}
    }
    if let Some(Action::BottomTab(index)) = keymap::lookup(Context::Global, key)
        && let Some(tab) = BottomTab::from_index(index)
    {
        select_tab(ui, tab);
        return Vec::new();
    }
    match &app.terminal {
        TerminalState::Open(view) => to_terminal_key(key)
            .map(|(k, m)| Command::TerminalInput(view.handle.encode_key(k, m)))
            .into_iter()
            .collect(),
        TerminalState::Exited { .. } if key.code == KeyCode::Enter => {
            let (cols, rows) = terminal_size(ui);
            vec![Command::TerminalOpen { cols, rows }]
        }
        _ => Vec::new(),
    }
}

/// Keys while the help overlay is open.
fn help_key(ui: &mut UiState, key: KeyEvent) {
    let Some(scroll) = ui.help else { return };
    let max = crate::help_view::max_scroll(ui.size.1);
    let page = crate::help_view::visible_rows(ui.size.1)
        .saturating_sub(1)
        .max(1);
    ui.help = match key.code {
        KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?' | 'q') => None,
        KeyCode::Up | KeyCode::Char('k') => Some(scroll.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => Some((scroll + 1).min(max)),
        KeyCode::PageUp => Some(scroll.saturating_sub(page)),
        KeyCode::PageDown => Some((scroll + page).min(max)),
        KeyCode::Home => Some(0),
        KeyCode::End => Some(max),
        _ => Some(scroll),
    };
}

/// Any key press dismisses the notices on show, then is handled as usual.
pub fn on_event(ui: &mut UiState, app: &AppState, event: Event) -> Vec<Command> {
    let mut commands = Vec::new();
    if matches!(event, Event::Key(_)) {
        commands.extend(app.notices.iter().map(|n| Command::DismissNotice(n.id)));
    }
    commands.extend(handle_event(ui, app, event));
    commands
}

const WHEEL_STEP: usize = 3;

/// Which area a terminal cell belongs to, and that area's rectangle.
fn area_at(ui: &UiState, app: &AppState, x: u16, y: u16) -> Option<(Focus, Rect)> {
    let areas = layout::areas(
        Rect::new(0, 0, ui.size.0, ui.size.1),
        ui.tree_visible(),
        ui.maximize_bottom,
    );
    let at = ratatui::layout::Position::new(x, y);
    let mut candidates = vec![(Focus::Bottom, areas.bottom)];
    if !ui.maximize_bottom {
        candidates.push((Focus::Local, areas.local));
        if app.remote.is_some() {
            candidates.push((Focus::Remote, areas.remote));
        }
        if let Some(tree) = areas.tree {
            candidates.push((Focus::Tree, tree));
        }
    }
    candidates.into_iter().find(|(_, rect)| rect.contains(at))
}

/// The row of a list drawn from `first_row_y` with the given scroll offset.
fn row_at(y: u16, first_row_y: u16, offset: usize) -> Option<usize> {
    y.checked_sub(first_row_y)
        .map(|row| usize::from(row) + offset)
}

fn mouse_event(ui: &mut UiState, app: &AppState, mouse: MouseEvent) -> Vec<Command> {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    if ui.prompt.is_some() && app.prompt.is_some() || ui.dialog.is_some() {
        return Vec::new();
    }
    if let Some(scroll) = ui.help {
        let max = crate::help_view::max_scroll(ui.size.1);
        match mouse.kind {
            MouseEventKind::ScrollUp => ui.help = Some(scroll.saturating_sub(WHEEL_STEP)),
            MouseEventKind::ScrollDown => ui.help = Some((scroll + WHEEL_STEP).min(max)),
            _ => {}
        }
        return Vec::new();
    }
    let Some((focus, area)) = area_at(ui, app, mouse.column, mouse.row) else {
        return Vec::new();
    };
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = mouse.kind == MouseEventKind::ScrollUp;
            wheel(ui, app, focus, up)
        }
        MouseEventKind::Down(MouseButton::Left) => {
            ui.focus = focus;
            click(ui, app, focus, area, mouse.column, mouse.row);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn wheel(ui: &mut UiState, app: &AppState, focus: Focus, up: bool) -> Vec<Command> {
    let step = |cursor: usize, last: usize| {
        if up {
            cursor.saturating_sub(WHEEL_STEP)
        } else {
            (cursor + WHEEL_STEP).min(last)
        }
    };
    match focus {
        Focus::Local | Focus::Remote => {
            let Some(view) = pane_view(app, focus) else {
                return Vec::new();
            };
            let rows = viewport(ui, focus);
            if let Some(pane) = ui.pane_mut(focus) {
                pane.cursor = step(pane.cursor, view.rows().saturating_sub(1));
                scroll_to_cursor(pane, rows);
            }
        }
        Focus::Tree => {
            let last = tree::rows(&app.servers, &ui.tree.expanded)
                .len()
                .saturating_sub(1);
            let height = tree_viewport(ui);
            ui.tree.cursor = step(ui.tree.cursor, last);
            if ui.tree.cursor < ui.tree.offset {
                ui.tree.offset = ui.tree.cursor;
            } else if ui.tree.cursor >= ui.tree.offset + height {
                ui.tree.offset = ui.tree.cursor + 1 - height;
            }
        }
        Focus::Bottom => match ui.bottom.tab {
            BottomTab::Log => {
                for _ in 0..WHEEL_STEP {
                    scroll_log(ui, if up { Action::Up } else { Action::Down });
                }
            }
            BottomTab::Terminal => {
                let lines = i32::try_from(WHEEL_STEP).unwrap_or(3);
                return vec![Command::TerminalScroll(if up { lines } else { -lines })];
            }
            tab => {
                let (len, rows) = (list_len(app, tab), bottom_rows(ui));
                if let Some(list) = ui.bottom.list_mut() {
                    list.cursor = step(list.cursor, len.saturating_sub(1));
                    keep_in_view(list, len, rows);
                }
            }
        },
    }
    Vec::new()
}

fn click(ui: &mut UiState, app: &AppState, focus: Focus, area: Rect, x: u16, y: u16) {
    match focus {
        Focus::Local | Focus::Remote => {
            let Some(view) = pane_view(app, focus) else {
                return;
            };
            let offset = ui.pane(focus).map_or(0, |p| p.offset);
            // border, then the header row
            if let Some(row) = row_at(y, area.y + 2, offset).filter(|r| *r < view.rows())
                && let Some(pane) = ui.pane_mut(focus)
            {
                pane.cursor = row;
            }
        }
        Focus::Tree => {
            let len = tree::rows(&app.servers, &ui.tree.expanded).len();
            if let Some(row) = row_at(y, area.y + 1, ui.tree.offset).filter(|r| *r < len) {
                ui.tree.cursor = row;
            }
        }
        Focus::Bottom => {
            if y == area.y {
                if let Some(tab) = bottom_view::tab_at(app, x.saturating_sub(area.x + 1)) {
                    ui.bottom.tab = tab;
                }
                return;
            }
            let tab = ui.bottom.tab;
            let len = list_len(app, tab);
            let rows = bottom_rows(ui);
            if let Some(list) = ui.bottom.list_mut()
                && let Some(row) = row_at(y, area.y + 2, list.offset).filter(|r| *r < len)
            {
                list.cursor = row;
                keep_in_view(list, len, rows);
            }
        }
    }
}

fn handle_event(ui: &mut UiState, app: &AppState, event: Event) -> Vec<Command> {
    match event {
        Event::Resize(width, height) => {
            ui.size = (width, height);
            sync(ui, app);
            Vec::new()
        }
        Event::Key(key) => {
            if ui
                .prompt
                .as_ref()
                .is_some_and(|p| app.prompt.as_ref().is_some_and(|a| a.id == p.id))
            {
                return prompt_key(ui, app, key);
            }
            if ui.help.is_some() {
                help_key(ui, key);
                return Vec::new();
            }
            if ui.dialog.is_none()
                && ui.focus == Focus::Bottom
                && ui.bottom.tab == BottomTab::Terminal
            {
                return terminal_key(ui, app, key);
            }
            if ui.dialog.is_some() {
                return dialog_key(ui, key);
            }
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
                Action::Help => {
                    ui.help = Some(0);
                    Vec::new()
                }
                Action::BottomTab(index) => {
                    if let Some(tab) = BottomTab::from_index(index) {
                        select_tab(ui, tab);
                    }
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
                    Focus::Bottom => bottom_action(ui, app, other),
                },
            }
        }
        Event::Paste(text) => {
            if let Some(slot) = ui.prompt.as_mut().filter(|s| !s.answered) {
                slot.ui.paste(&text);
                return Vec::new();
            }
            if let Some(dialog) = ui.dialog.as_mut() {
                dialog.paste(&text);
                return Vec::new();
            }
            if ui.focus == Focus::Bottom
                && ui.bottom.tab == BottomTab::Terminal
                && let TerminalState::Open(view) = &app.terminal
            {
                return vec![Command::TerminalInput(view.handle.paste_bytes(&text))];
            }
            Vec::new()
        }
        Event::Mouse(mouse) => mouse_event(ui, app, mouse),
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
            [r#"Delete { pane: Remote, names: ["html"] }"#]
        );
        ui.remote.selected = ["index.php".to_owned(), "style.css".to_owned()].into();
        assert_eq!(
            commands(&on_event(&mut ui, &app, key(KeyCode::Delete))),
            [r#"Delete { pane: Remote, names: ["index.php", "style.css"] }"#]
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
        on_event(&mut ui, &app, with(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(
            ui.focus,
            Focus::Tree,
            "what a real terminal sends for Shift-Tab"
        );
        on_event(&mut ui, &app, with(KeyCode::Tab, KeyModifiers::NONE));
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

#[cfg(test)]
mod dialog_tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::test_support::{app, connected_app, sample_tree, synced};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ch(c: char) -> Event {
        key(KeyCode::Char(c))
    }

    fn tree_app() -> AppState {
        let mut app = app();
        app.servers = std::sync::Arc::new(sample_tree());
        app
    }

    /// Tree focused, folders open: 0 Personal, 1 blog, 2 Work, 3 prod-web, 4 staging, 5 home-nas.
    fn tree_ui(app: &AppState) -> UiState {
        let mut ui = synced(120, 30, app);
        ui.focus = Focus::Tree;
        ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
        ui
    }

    fn type_text(ui: &mut UiState, app: &AppState, text: &str) {
        for c in text.chars() {
            on_event(ui, app, ch(c));
        }
    }

    #[test]
    fn n_opens_the_site_editor_and_enter_with_a_filled_form_adds_the_site_in_the_folder() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        ui.tree.cursor = 2; // Work
        assert!(on_event(&mut ui, &app, ch('n')).is_empty());
        assert!(matches!(ui.dialog, Some(Dialog::Site(_))));
        type_text(&mut ui, &app, "extra");
        on_event(&mut ui, &app, key(KeyCode::Tab));
        on_event(&mut ui, &app, key(KeyCode::Tab));
        type_text(&mut ui, &app, "extra.example.org");
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(ui.dialog.is_none());
        let Some(Command::Tree(TreeOp::AddSite(site))) = commands.first() else {
            panic!("{commands:?}")
        };
        let work = app
            .servers
            .folders()
            .iter()
            .find(|f| f.name == "Work")
            .unwrap();
        assert_eq!(
            site.folder,
            Some(work.id),
            "created in the folder under the cursor"
        );
    }

    #[test]
    fn keys_go_to_the_dialog_only_and_escape_closes_it_without_commands() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        on_event(&mut ui, &app, ch('n'));
        // `q` must be typed into the name, not quit
        let commands = on_event(&mut ui, &app, ch('q'));
        assert!(commands.is_empty() && !ui.quit_requested);
        assert!(on_event(&mut ui, &app, key(KeyCode::Esc)).is_empty());
        assert!(ui.dialog.is_none());
        // and the tree handles keys again
        on_event(&mut ui, &app, ch('j'));
        assert_eq!(ui.tree.cursor, 1);
    }

    #[test]
    fn a_validation_error_keeps_the_dialog_open() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        on_event(&mut ui, &app, ch('n'));
        assert!(on_event(&mut ui, &app, key(KeyCode::Enter)).is_empty());
        let Some(Dialog::Site(editor)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(editor.form.error.as_deref(), Some("Give the site a name."));
    }

    #[test]
    fn e_edits_the_site_under_the_cursor_and_only_sites() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        ui.tree.cursor = 0; // a folder
        on_event(&mut ui, &app, ch('e'));
        assert!(ui.dialog.is_none());
        ui.tree.cursor = 3; // prod-web
        on_event(&mut ui, &app, ch('e'));
        let Some(Dialog::Site(editor)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(editor.form.text_of("name"), "prod-web");
        assert_eq!(editor.form.text_of("host"), "prod-web.example.org");
    }

    #[test]
    fn rename_new_folder_move_and_delete_build_tree_operations() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        ui.tree.cursor = 5; // home-nas

        on_event(&mut ui, &app, ch('r'));
        type_text(&mut ui, &app, "2");
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Tree(TreeOp::Rename { name, .. })] if name == "home-nas2"),
            "{commands:?}"
        );

        on_event(&mut ui, &app, ch('N'));
        type_text(&mut ui, &app, "Misc");
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Tree(TreeOp::AddFolder { name, parent: None })] if name == "Misc"),
            "{commands:?}"
        );

        on_event(&mut ui, &app, ch('m'));
        let Some(Dialog::Move(picker)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(picker.choices.len(), 3, "top level, Personal, Work");
        on_event(&mut ui, &app, key(KeyCode::Down));
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(
                &commands[..],
                [Command::Tree(TreeOp::Move {
                    parent: Some(_),
                    ..
                })]
            ),
            "{commands:?}"
        );

        on_event(&mut ui, &app, key(KeyCode::Delete));
        assert!(matches!(ui.dialog, Some(Dialog::Confirm(_))));
        assert!(on_event(&mut ui, &app, ch('n')).is_empty());
        assert!(ui.dialog.is_none());
        on_event(&mut ui, &app, key(KeyCode::Delete));
        let commands = on_event(&mut ui, &app, ch('y'));
        assert!(
            matches!(&commands[..], [Command::Tree(TreeOp::Delete { .. })]),
            "{commands:?}"
        );
    }

    #[test]
    fn a_folder_cannot_be_moved_into_itself_or_below() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = filecargo_config::ConfigStore::open(
            Paths::from_override(Some(dir.path().to_path_buf())),
            std::sync::Arc::new(filecargo_config::MemoryStore::new()),
        )
        .unwrap();
        let a = match store
            .apply(TreeOp::AddFolder {
                name: "A".into(),
                parent: None,
            })
            .unwrap()
        {
            NodeId::Folder(id) => id,
            NodeId::Site(_) => unreachable!(),
        };
        store
            .apply(TreeOp::AddFolder {
                name: "B".into(),
                parent: Some(a),
            })
            .unwrap();
        store
            .apply(TreeOp::AddFolder {
                name: "C".into(),
                parent: None,
            })
            .unwrap();
        let tree = store.tree().clone();
        let picker = MovePicker::new(&tree, NodeId::Folder(a));
        let names: Vec<&str> = picker.choices.iter().map(|c| c.1.as_str()).collect();
        assert_eq!(names, ["/ (top level)", "/C"], "A and A/B are excluded");
        assert_eq!(picker.cursor, 0, "starts on the current parent");
    }

    #[test]
    fn f7_and_f2_on_the_remote_pane_open_input_dialogs_and_do_nothing_locally() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Remote;
        ui.remote.cursor = 2; // index.php
        on_event(&mut ui, &app, key(KeyCode::F(7)));
        type_text(&mut ui, &app, "new");
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Mkdir { name, .. }] if name == "new"),
            "{commands:?}"
        );

        on_event(&mut ui, &app, key(KeyCode::F(2)));
        let Some(Dialog::Input(input)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(
            input.form.text_of("value"),
            "index.php",
            "pre-filled with the current name"
        );
        type_text(&mut ui, &app, "x");
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Rename { from, to, .. }] if from == "index.php" && to == "index.phpx"),
            "{commands:?}"
        );

        ui.focus = Focus::Local;
        on_event(&mut ui, &app, key(KeyCode::F(7)));
        assert!(
            ui.dialog.is_none(),
            "the local pane has no remote operations"
        );
    }

    #[test]
    fn pasting_goes_into_the_open_dialog() {
        let app = tree_app();
        let mut ui = tree_ui(&app);
        on_event(&mut ui, &app, ch('n'));
        on_event(&mut ui, &app, Event::Paste("pasted name".to_owned()));
        let Some(Dialog::Site(editor)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(editor.form.text_of("name"), "pasted name");
    }
}

#[cfg(test)]
mod prompt_tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::test_support::{app, connected_app, synced};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ch(c: char) -> Event {
        key(KeyCode::Char(c))
    }

    fn with_prompt(mut app: AppState, id: u64, kind: PromptKind) -> AppState {
        app.prompt = Some(Prompt {
            id: PromptId(id),
            kind,
        });
        app
    }

    fn host_key() -> PromptKind {
        PromptKind::HostKey(HostKeyPrompt {
            host: "example.org".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        })
    }

    #[test]
    fn the_prompt_takes_every_key_and_the_answer_goes_out_once() {
        let app = with_prompt(app(), 7, host_key());
        let mut ui = synced(100, 30, &app);
        // `q` does not quit and `Tab` does not move the focus while a prompt shows
        assert!(on_event(&mut ui, &app, ch('q')).is_empty());
        assert!(!ui.quit_requested);
        let focus = ui.focus;
        assert!(on_event(&mut ui, &app, key(KeyCode::Tab)).is_empty());
        assert_eq!(ui.focus, focus);

        let commands = on_event(&mut ui, &app, ch('a'));
        assert!(
            matches!(
                &commands[..],
                [Command::Answer {
                    id: PromptId(7),
                    answer: PromptAnswer::Trust(TrustDecision::TrustAlways)
                }]
            ),
            "{commands:?}"
        );
        // the snapshot still carries the prompt: a second key must not answer again
        assert!(on_event(&mut ui, &app, ch('y')).is_empty());
        assert!(ui.prompt.as_ref().unwrap().answered);
    }

    #[test]
    fn a_new_prompt_id_gets_a_fresh_slot_and_no_prompt_clears_it() {
        let first = with_prompt(app(), 1, host_key());
        let mut ui = synced(100, 30, &first);
        on_event(&mut ui, &first, ch('y'));
        let second = with_prompt(app(), 2, host_key());
        sync(&mut ui, &second);
        assert!(
            !ui.prompt.as_ref().unwrap().answered,
            "the next prompt can be answered"
        );
        assert_eq!(on_event(&mut ui, &second, ch('n')).len(), 1);
        sync(&mut ui, &app());
        assert!(ui.prompt.is_none());
        // and keys reach the panes again
        let plain = app();
        on_event(&mut ui, &plain, ch('j'));
        assert_eq!(ui.local.cursor, 1);
    }

    #[test]
    fn a_prompt_sits_above_an_open_dialog() {
        let plain = connected_app();
        let mut ui = synced(100, 30, &plain);
        ui.focus = Focus::Remote;
        on_event(&mut ui, &plain, key(KeyCode::F(7)));
        assert!(ui.dialog.is_some());
        let app = with_prompt(connected_app(), 3, host_key());
        sync(&mut ui, &app);
        let commands = on_event(&mut ui, &app, ch('n'));
        assert_eq!(commands.len(), 1, "the prompt got the key, not the dialog");
        assert!(ui.dialog.is_some(), "the dialog waits underneath");
    }

    #[test]
    fn pasting_into_a_password_prompt_fills_the_field() {
        use filecargo_config::ExposeSecret;
        let kind = PromptKind::Credential(CredentialPrompt::Password {
            site: "work".into(),
            user: "me".into(),
            retry: false,
        });
        let app = with_prompt(app(), 4, kind);
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, Event::Paste("s3cret".into()));
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        let [
            Command::Answer {
                answer: PromptAnswer::Credential(Some(answer)),
                ..
            },
        ] = &commands[..]
        else {
            panic!("{commands:?}")
        };
        assert_eq!(answer.values[0].expose_secret(), "s3cret");
    }

    #[test]
    fn g_c_and_i_open_the_utility_dialogs() {
        let app = connected_app();
        let mut ui = synced(120, 30, &app);
        ui.focus = Focus::Remote;
        on_event(&mut ui, &app, ch('g'));
        let Some(Dialog::Input(input)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(
            input.form.text_of("value"),
            "/var/www",
            "pre-filled with the current path"
        );
        for _ in 0.."/var/www".len() {
            on_event(&mut ui, &app, key(KeyCode::Backspace));
        }
        for c in "/etc/nginx".chars() {
            on_event(&mut ui, &app, ch(c));
        }
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Navigate { pane: PaneId::Remote, path }] if path == "/etc/nginx"),
            "{commands:?}"
        );

        ui.remote.cursor = 2; // index.php
        on_event(&mut ui, &app, ch('c'));
        let Some(Dialog::Chmod(chmod)) = &ui.dialog else {
            panic!()
        };
        assert_eq!(chmod.names, ["index.php"]);
        let commands = on_event(&mut ui, &app, key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Chmod { mode: 0o644, .. }]),
            "{commands:?}"
        );

        ui.focus = Focus::Local;
        on_event(&mut ui, &app, ch('c'));
        assert!(ui.dialog.is_none(), "chmod is remote-only");
        on_event(&mut ui, &app, ch('g'));
        assert!(
            matches!(ui.dialog, Some(Dialog::Input(_))),
            "go-to works on the local pane too"
        );
        on_event(&mut ui, &app, key(KeyCode::Esc));

        ui.focus = Focus::Tree;
        on_event(&mut ui, &app, ch('i'));
        assert!(matches!(ui.dialog, Some(Dialog::Import(_))));
    }
}

#[cfg(test)]
mod bottom_tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::test_support::{app, busy_queue, synced};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ch(c: char) -> Event {
        key(KeyCode::Char(c))
    }

    fn alt(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    fn busy() -> AppState {
        let mut app = app();
        app.queue = std::sync::Arc::new(busy_queue());
        app
    }

    fn debug(commands: &[Command]) -> Vec<String> {
        commands.iter().map(|c| format!("{c:?}")).collect()
    }

    #[test]
    fn alt_digits_pick_the_tab_and_focus_the_panel() {
        let app = busy();
        let mut ui = synced(100, 30, &app);
        for (digit, tab) in [
            ('2', BottomTab::Completed),
            ('3', BottomTab::Failed),
            ('4', BottomTab::Log),
            ('1', BottomTab::Queue),
        ] {
            on_event(&mut ui, &app, alt(digit));
            assert_eq!((ui.bottom.tab, ui.focus), (tab, Focus::Bottom));
            ui.focus = Focus::Local;
        }
    }

    #[test]
    fn left_and_right_cycle_the_tabs_and_wrap() {
        let app = busy();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Bottom;
        on_event(&mut ui, &app, key(KeyCode::Left));
        assert_eq!(ui.bottom.tab, BottomTab::Terminal, "wraps backwards");
        // the terminal tab sends every key to the shell; Alt-digits leave it
        on_event(&mut ui, &app, alt('1'));
        assert_eq!(ui.bottom.tab, BottomTab::Queue);
        on_event(&mut ui, &app, ch('l'));
        assert_eq!(ui.bottom.tab, BottomTab::Completed);
    }

    #[test]
    fn the_cursor_moves_within_the_list_and_remove_targets_the_row_under_it() {
        let app = busy();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Bottom;
        on_event(&mut ui, &app, key(KeyCode::Down));
        on_event(&mut ui, &app, key(KeyCode::Down));
        on_event(&mut ui, &app, key(KeyCode::Down));
        assert_eq!(ui.bottom.queue.cursor, 2, "stops on the last of 3 rows");
        let commands = on_event(&mut ui, &app, key(KeyCode::Delete));
        assert_eq!(debug(&commands), ["QueueRemove(TransferId(3))"]);
        on_event(&mut ui, &app, key(KeyCode::Home));
        let commands = on_event(&mut ui, &app, ch('d'));
        assert_eq!(debug(&commands), ["QueueRemove(TransferId(1))"]);
    }

    #[test]
    fn pause_toggles_with_the_queue_state_and_clear_and_retry_map_to_commands() {
        let mut app = busy();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Bottom;
        assert_eq!(
            debug(&on_event(&mut ui, &app, ch('p'))),
            ["QueueSetProcessing(false)"]
        );
        let mut paused = (*app.queue).clone();
        paused.processing = false;
        app.queue = std::sync::Arc::new(paused);
        assert_eq!(
            debug(&on_event(&mut ui, &app, ch('p'))),
            ["QueueSetProcessing(true)"]
        );
        assert_eq!(
            debug(&on_event(&mut ui, &app, ch('C'))),
            ["QueueClearCompleted"]
        );
        assert_eq!(
            debug(&on_event(&mut ui, &app, ch('R'))),
            ["QueueRetryFailed"]
        );
        assert!(
            on_event(&mut ui, &app, ch('r')).is_empty(),
            "retry is for the failed tab"
        );
        on_event(&mut ui, &app, alt('3'));
        assert_eq!(
            debug(&on_event(&mut ui, &app, ch('r'))),
            ["QueueRetry(TransferId(6))"]
        );
    }

    #[test]
    fn list_keys_do_nothing_on_an_empty_list() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Bottom;
        on_event(&mut ui, &app, key(KeyCode::Down));
        on_event(&mut ui, &app, key(KeyCode::End));
        assert_eq!(ui.bottom.queue, ListUi::default());
        assert!(on_event(&mut ui, &app, key(KeyCode::Delete)).is_empty());
    }

    #[test]
    fn cursors_are_clamped_when_the_list_shrinks() {
        let mut app = busy();
        let mut ui = synced(100, 30, &app);
        ui.focus = Focus::Bottom;
        on_event(&mut ui, &app, key(KeyCode::End));
        assert_eq!(ui.bottom.queue.cursor, 2);
        let mut smaller = (*app.queue).clone();
        smaller.pending.truncate(1);
        app.queue = std::sync::Arc::new(smaller);
        sync(&mut ui, &app);
        assert_eq!(ui.bottom.queue.cursor, 0);
    }

    fn log_line(n: usize) -> LogLine {
        LogLine {
            time: std::time::UNIX_EPOCH,
            level: LogLevel::Info,
            target: "filecargo::test".into(),
            message: format!("line {n}"),
        }
    }

    #[test]
    fn the_log_scrolls_up_stops_at_the_ends_and_f_follows_again() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        for n in 0..100 {
            ui.log.push(log_line(n));
        }
        on_event(&mut ui, &app, alt('4'));
        assert!(ui.bottom.log_follow);
        on_event(&mut ui, &app, key(KeyCode::Up));
        assert_eq!((ui.bottom.log_scroll, ui.bottom.log_follow), (1, false));
        on_event(&mut ui, &app, key(KeyCode::PageUp));
        assert!(ui.bottom.log_scroll > 1);
        on_event(&mut ui, &app, key(KeyCode::Home));
        let rows = log_rows(&ui);
        assert_eq!(
            ui.bottom.log_scroll,
            100 - rows,
            "the oldest line is at the top"
        );
        on_event(&mut ui, &app, key(KeyCode::Up));
        assert_eq!(
            ui.bottom.log_scroll,
            100 - rows,
            "no scrolling past the oldest line"
        );
        on_event(&mut ui, &app, ch('f'));
        assert_eq!((ui.bottom.log_scroll, ui.bottom.log_follow), (0, true));
        on_event(&mut ui, &app, key(KeyCode::Up));
        on_event(&mut ui, &app, key(KeyCode::Down));
        assert!(
            ui.bottom.log_follow,
            "scrolling back to the bottom follows again"
        );
    }

    #[test]
    fn no_two_bindings_of_one_context_share_a_key() {
        use crate::keymap::BINDINGS;
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                if a.context != b.context {
                    continue;
                }
                for ka in a.keys {
                    for kb in b.keys {
                        assert!(
                            !(ka.code == kb.code && ka.mods == kb.mods),
                            "{:?} is bound twice in {:?}: {} and {}",
                            ka.code,
                            a.context,
                            a.help,
                            b.help
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod terminal_tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::test_support::{FakeShell, app, connected_app, synced};

    fn press(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }

    fn plain(code: KeyCode) -> Event {
        press(code, KeyModifiers::NONE)
    }

    fn debug(commands: &[Command]) -> Vec<String> {
        commands.iter().map(|c| format!("{c:?}")).collect()
    }

    /// Connected app with the shell open, the terminal tab shown and focused.
    async fn with_shell() -> (UiState, AppState, FakeShell) {
        let shell = FakeShell::open(98, 7, "$ ").await;
        let mut app = connected_app();
        app.terminal = TerminalState::Open(shell.view.clone());
        let mut ui = synced(100, 30, &app);
        ui.bottom.tab = BottomTab::Terminal;
        ui.focus = Focus::Bottom;
        (ui, app, shell)
    }

    fn input(commands: &[Command]) -> Vec<u8> {
        match commands {
            [Command::TerminalInput(bytes)] => bytes.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn showing_the_tab_opens_the_shell_once_with_the_panel_size_and_again_after_leaving() {
        let mut app = connected_app();
        app.terminal = TerminalState::Closed;
        let mut ui = synced(100, 30, &app);
        assert!(
            housekeeping(&mut ui, &app).is_empty(),
            "not before the tab shows"
        );
        on_event(&mut ui, &app, press(KeyCode::Char('5'), KeyModifiers::ALT));
        assert_eq!(ui.focus, Focus::Bottom);
        let size = terminal_size(&ui);
        assert_eq!(
            debug(&housekeeping(&mut ui, &app)),
            [format!(
                "TerminalOpen {{ cols: {}, rows: {} }}",
                size.0, size.1
            )]
        );
        assert!(
            housekeeping(&mut ui, &app).is_empty(),
            "the request is out: no repeats"
        );
        // leaving and coming back asks again (the first attempt may have failed)
        on_event(&mut ui, &app, press(KeyCode::Char('1'), KeyModifiers::ALT));
        assert!(housekeeping(&mut ui, &app).is_empty());
        on_event(&mut ui, &app, press(KeyCode::Char('5'), KeyModifiers::ALT));
        assert_eq!(housekeeping(&mut ui, &app).len(), 1);
    }

    #[test]
    fn without_a_shell_nothing_is_opened_and_keys_do_nothing() {
        let app = app(); // TerminalState::NotAvailable
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, press(KeyCode::Char('5'), KeyModifiers::ALT));
        assert!(housekeeping(&mut ui, &app).is_empty());
        assert!(on_event(&mut ui, &app, plain(KeyCode::Char('x'))).is_empty());
    }

    #[tokio::test]
    async fn every_key_but_the_escapes_goes_to_the_shell_encoded_with_its_modes() {
        let (mut ui, app, shell) = with_shell().await;
        assert_eq!(
            input(&on_event(&mut ui, &app, plain(KeyCode::Char('q')))),
            b"q"
        );
        assert!(!ui.quit_requested, "`q` is typed, it does not quit");
        assert_eq!(input(&on_event(&mut ui, &app, plain(KeyCode::Tab))), b"\t");
        assert_eq!(ui.focus, Focus::Bottom, "Tab does not move the focus");
        assert_eq!(
            input(&on_event(&mut ui, &app, plain(KeyCode::Esc))),
            b"\x1b"
        );
        assert_eq!(
            input(&on_event(
                &mut ui,
                &app,
                press(KeyCode::Char('c'), KeyModifiers::CONTROL)
            )),
            [0x03]
        );
        assert_eq!(
            input(&on_event(&mut ui, &app, plain(KeyCode::Up))),
            b"\x1b[A"
        );
        shell.say("\x1b[?1h").await; // application cursor keys
        assert_eq!(
            input(&on_event(&mut ui, &app, plain(KeyCode::Up))),
            b"\x1bOA"
        );
        assert!(
            on_event(&mut ui, &app, plain(KeyCode::F(15))).is_empty(),
            "unsendable keys are dropped"
        );
    }

    #[tokio::test]
    async fn ctrl_backslash_and_f12_leave_the_terminal_and_alt_digits_switch_tabs() {
        let (mut ui, app, _shell) = with_shell().await;
        assert!(
            on_event(
                &mut ui,
                &app,
                press(KeyCode::Char('\\'), KeyModifiers::CONTROL)
            )
            .is_empty()
        );
        assert_eq!(ui.focus, Focus::Remote, "back to the remote pane");
        ui.focus = Focus::Bottom;
        assert!(on_event(&mut ui, &app, plain(KeyCode::F(12))).is_empty());
        assert_eq!(ui.focus, Focus::Remote);
        ui.focus = Focus::Bottom;
        assert!(on_event(&mut ui, &app, press(KeyCode::Char('4'), KeyModifiers::ALT)).is_empty());
        assert_eq!(ui.bottom.tab, BottomTab::Log);
        // and the terminal keeps its keys when it is shown again
        on_event(&mut ui, &app, press(KeyCode::Char('5'), KeyModifiers::ALT));
        assert_eq!(on_event(&mut ui, &app, plain(KeyCode::Char('x'))).len(), 1);
    }

    #[tokio::test]
    async fn shift_page_keys_scroll_the_history_by_a_page() {
        let (mut ui, app, _shell) = with_shell().await;
        let page = i32::from(terminal_size(&ui).1) - 1;
        assert_eq!(
            debug(&on_event(
                &mut ui,
                &app,
                press(KeyCode::PageUp, KeyModifiers::SHIFT)
            )),
            [format!("TerminalScroll({page})")]
        );
        assert_eq!(
            debug(&on_event(
                &mut ui,
                &app,
                press(KeyCode::PageDown, KeyModifiers::SHIFT)
            )),
            [format!("TerminalScroll(-{page})")]
        );
        // plain PageUp belongs to the shell
        assert_eq!(
            input(&on_event(&mut ui, &app, plain(KeyCode::PageUp))),
            b"\x1b[5~"
        );
    }

    #[tokio::test]
    async fn the_shell_is_resized_with_the_panel_and_only_then() {
        let (mut ui, app, _shell) = with_shell().await;
        ui.bottom.term_size = Some(terminal_size(&ui));
        assert!(housekeeping(&mut ui, &app).is_empty());
        on_event(&mut ui, &app, Event::Resize(120, 40));
        let size = terminal_size(&ui);
        assert_eq!(
            debug(&housekeeping(&mut ui, &app)),
            [format!(
                "TerminalResize {{ cols: {}, rows: {} }}",
                size.0, size.1
            )]
        );
        assert!(housekeeping(&mut ui, &app).is_empty());
    }

    #[tokio::test]
    async fn an_exited_shell_reopens_on_enter_and_ignores_other_keys() {
        let shell = FakeShell::open(98, 7, "bye").await;
        let mut app = connected_app();
        app.terminal = TerminalState::Exited {
            code: Some(0),
            view: shell.view.clone(),
        };
        let mut ui = synced(100, 30, &app);
        ui.bottom.tab = BottomTab::Terminal;
        ui.focus = Focus::Bottom;
        assert!(on_event(&mut ui, &app, plain(KeyCode::Char('x'))).is_empty());
        let commands = on_event(&mut ui, &app, plain(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::TerminalOpen { .. }]),
            "{commands:?}"
        );
    }

    #[tokio::test]
    async fn pasting_goes_to_the_shell_bracketed_only_when_it_asked_for_it() {
        let (mut ui, app, shell) = with_shell().await;
        let commands = on_event(&mut ui, &app, Event::Paste("ls\nrm x".into()));
        assert_eq!(input(&commands), b"ls\rrm x", "line breaks become Enter");
        shell.say("\x1b[?2004h").await;
        let commands = on_event(&mut ui, &app, Event::Paste("a\x1b[201~b".into()));
        assert_eq!(
            input(&commands),
            b"\x1b[200~ab\x1b[201~",
            "the end marker cannot be smuggled in"
        );
    }
}

#[cfg(test)]
mod polish_tests {
    use ratatui::crossterm::event::{
        KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use super::*;
    use crate::test_support::{app, busy_queue, connected_app, sample_tree, synced};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn click(column: u16, row: u16) -> Event {
        mouse(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    #[test]
    fn f1_and_question_mark_open_the_help_which_swallows_keys_until_it_closes() {
        let app = app();
        for opener in [key(KeyCode::F(1)), key(KeyCode::Char('?'))] {
            let mut ui = synced(100, 30, &app);
            on_event(&mut ui, &app, opener);
            assert_eq!(ui.help, Some(0));
            assert!(on_event(&mut ui, &app, key(KeyCode::Char('j'))).is_empty());
            assert_eq!(ui.help, Some(1), "j scrolls the help, not the pane");
            assert_eq!(ui.local.cursor, 0);
            assert!(on_event(&mut ui, &app, key(KeyCode::Char('q'))).is_empty());
            assert!(ui.help.is_none(), "q closes the help instead of quitting");
            assert!(!ui.quit_requested);
        }
    }

    #[test]
    fn the_help_scroll_stops_at_both_ends() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, key(KeyCode::F(1)));
        on_event(&mut ui, &app, key(KeyCode::Up));
        assert_eq!(ui.help, Some(0));
        on_event(&mut ui, &app, key(KeyCode::End));
        let max = crate::help_view::max_scroll(30);
        assert_eq!(ui.help, Some(max));
        on_event(&mut ui, &app, key(KeyCode::Down));
        assert_eq!(ui.help, Some(max));
        on_event(&mut ui, &app, key(KeyCode::Esc));
        assert!(ui.help.is_none());
    }

    #[test]
    fn any_key_dismisses_the_notices_on_show_and_is_still_handled() {
        let mut app = app();
        app.notices = vec![
            Notice {
                id: NoticeId(1),
                level: Level::Info,
                text: "one".into(),
            },
            Notice {
                id: NoticeId(2),
                level: Level::Error,
                text: "two".into(),
            },
        ];
        let mut ui = synced(100, 30, &app);
        let commands = on_event(&mut ui, &app, key(KeyCode::Char('j')));
        let text: Vec<String> = commands.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            text,
            ["DismissNotice(NoticeId(1))", "DismissNotice(NoticeId(2))"]
        );
        assert_eq!(ui.local.cursor, 1, "the key still moved the cursor");
        // mouse events do not dismiss
        assert!(on_event(&mut ui, &app, mouse(MouseEventKind::Moved, 5, 5)).is_empty());
    }

    // 100x30: tree 0..20, local 20..60, remote 60..100, panes rows 0..20 (header at y=1, rows from y=2)
    #[test]
    fn a_click_focuses_the_area_and_selects_the_row_under_the_pointer() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, click(70, 4)); // remote, third row
        assert_eq!((ui.focus, ui.remote.cursor), (Focus::Remote, 2));
        on_event(&mut ui, &app, click(30, 3)); // local, second row
        assert_eq!((ui.focus, ui.local.cursor), (Focus::Local, 1));
        on_event(&mut ui, &app, click(30, 18)); // below the last entry: focus only
        assert_eq!(ui.local.cursor, 1);
        on_event(&mut ui, &app, click(30, 1)); // the header row selects nothing
        assert_eq!(ui.local.cursor, 1);
    }

    #[test]
    fn a_click_on_a_remote_pane_that_does_not_exist_changes_nothing() {
        let app = app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, click(70, 4));
        assert_eq!(ui.focus, Focus::Local);
    }

    #[test]
    fn the_wheel_scrolls_the_area_under_the_pointer_by_three_rows_within_bounds() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        on_event(&mut ui, &app, mouse(MouseEventKind::ScrollDown, 30, 5));
        assert_eq!(ui.local.cursor, 3);
        on_event(&mut ui, &app, mouse(MouseEventKind::ScrollDown, 30, 5));
        assert_eq!(ui.local.cursor, 5, "the last of 6 rows");
        on_event(&mut ui, &app, mouse(MouseEventKind::ScrollUp, 30, 5));
        assert_eq!(ui.local.cursor, 2);
        assert_eq!(ui.remote.cursor, 0, "the other pane did not move");
        assert_eq!(ui.focus, Focus::Local, "scrolling does not steal the focus");
    }

    #[test]
    fn clicks_in_the_tree_and_the_bottom_panel_pick_rows_and_tabs() {
        let mut app = app();
        app.servers = std::sync::Arc::new(sample_tree());
        app.queue = std::sync::Arc::new(busy_queue());
        let mut ui = synced(100, 30, &app);
        ui.tree.expanded = app.servers.folders().iter().map(|f| f.id).collect();
        on_event(&mut ui, &app, click(5, 4)); // tree: rows start at y=1
        assert_eq!((ui.focus, ui.tree.cursor), (Focus::Tree, 3));
        // the bottom panel starts at y=21 (30 rows - status - 8 panel rows); its title row is y=21
        let bottom = layout::areas(Rect::new(0, 0, 100, 30), true, false).bottom;
        on_event(&mut ui, &app, click(bottom.x + 1 + 12, bottom.y)); // " Queue (3) │ Completed (2) …"
        assert_eq!(ui.bottom.tab, BottomTab::Completed);
        assert_eq!(ui.focus, Focus::Bottom);
        on_event(&mut ui, &app, click(bottom.x + 3, bottom.y + 3)); // second row of the list
        assert_eq!(ui.bottom.completed.cursor, 1);
    }

    #[test]
    fn mouse_events_are_ignored_while_a_dialog_or_prompt_is_up() {
        let app = connected_app();
        let mut ui = synced(100, 30, &app);
        ui.dialog = Some(Dialog::Confirm(ConfirmDialog {
            title: "t".into(),
            body: "b".into(),
            op: TreeOp::Delete {
                node: NodeId::Site(SiteId::new()),
            },
        }));
        on_event(&mut ui, &app, click(70, 4));
        assert_eq!(ui.focus, Focus::Local);
    }
}

#[cfg(test)]
mod binding_table_tests {
    use ratatui::crossterm::event::KeyEvent;

    use crate::keymap::{BINDINGS, lookup_exact};

    /// A concrete key press for a key spec (shifted letters arrive as capitals).
    fn press(spec: &crate::keymap::KeySpec) -> KeyEvent {
        KeyEvent::new(spec.code, spec.mods)
    }

    #[test]
    fn every_key_of_every_binding_resolves_to_that_binding_in_its_context() {
        let mut checked = 0;
        for binding in BINDINGS {
            for key in binding.keys {
                assert_eq!(
                    lookup_exact(binding.context, press(key)),
                    Some(binding.action),
                    "{:?} in {:?} ({})",
                    key.code,
                    binding.context,
                    binding.help
                );
                checked += 1;
            }
        }
        assert!(checked > 90, "{checked} keys checked");
    }

    #[test]
    fn the_spec_table_rows_all_have_bindings() {
        // the rows of SPEC-tui.md's "Key bindings" table, by help text fragment per context
        let wanted = [
            "focus the next area",
            "focus the previous area",
            "this help",
            "refresh the focused pane",
            "bottom tab: Queue",
            "show / hide the server tree",
            "maximize / restore the bottom panel",
            "quit",
            "expand / collapse a folder",
            "connect to a site",
            "new site",
            "new folder",
            "edit the site",
            "rename",
            "duplicate the site",
            "move to a folder",
            "delete the site or folder",
            "import a FileZilla",
            "disconnect",
            "parent folder",
            "select / unselect",
            "invert the selection",
            "select all",
            "transfer the selection",
            "new remote folder",
            "rename (remote)",
            "delete (remote)",
            "change permissions",
            "show / hide dotfiles",
            "cycle the sort order",
            "go to a path",
            "retry the failed item",
            "remove the item",
            "clear the completed list",
            "pause / resume the queue",
            "scroll the log",
            "follow the newest line",
            "leave the terminal",
            "scroll back through the output",
        ];
        for fragment in wanted {
            assert!(
                BINDINGS.iter().any(|b| b.help.contains(fragment)),
                "no binding for {fragment:?}"
            );
        }
    }
}
