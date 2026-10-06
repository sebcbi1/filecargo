//! A file pane: path bar, a `DataTable` with sortable columns, and a multi-selection that lives
//! in the table delegate (gpui-kit's table has a single selected row).

use std::collections::BTreeSet;
use std::sync::Arc;

use filecargo_app_core::prelude::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, ParentElement as _, Render, Stateful, Styled as _, Subscription,
    WeakEntity, Window, div,
};

use crate::format::{format_permissions, format_size, format_time};
use crate::model::AppModel;

gpui_kit::actions!(
    filecargo_pane,
    [
        OpenRow,
        ParentDir,
        SelectAllRows,
        RefreshPane,
        TransferSelection,
        NewRemoteFolder,
        RenameEntry,
        DeleteEntries,
        FocusPath
    ]
);

/// The pane's listing as the app published it, reduced to what the table needs.
pub struct PaneData {
    pub id: PaneId,
    pub path: String,
    pub has_parent: bool,
    pub entries: Arc<[Entry]>,
    pub generation: u64,
    pub sort: Sort,
    pub error: Option<String>,
}

pub fn pane_data(state: &AppState, id: PaneId) -> Option<PaneData> {
    match id {
        PaneId::Local => Some(PaneData {
            id,
            path: state.local.path.display().to_string(),
            has_parent: state.local.path.parent().is_some(),
            entries: state.local.entries.clone(),
            generation: state.local.generation,
            sort: state.local.sort,
            error: state.local.error.clone(),
        }),
        PaneId::Remote => state.remote.as_ref().map(|pane| PaneData {
            id,
            path: pane.path.to_string(),
            has_parent: pane.path.parent().is_some(),
            entries: pane.entries.clone(),
            generation: pane.generation,
            sort: pane.sort,
            error: pane.error.clone(),
        }),
    }
}

const COLUMNS: [(&str, f32); 4] = [
    ("Name", 260.0),
    ("Size", 80.0),
    ("Modified", 140.0),
    ("Permissions", 110.0),
];

pub struct PaneDelegate {
    pane: PaneId,
    handle: AppHandle,
    pub entries: Arc<[Entry]>,
    pub has_parent: bool,
    pub sort: Sort,
    /// Names of the selected entries (a set of names, so it survives refreshes).
    pub selected: BTreeSet<String>,
    anchor: Option<usize>,
    /// A mouse press chose the selection just now; the `SelectRow` event that follows it must
    /// not override it (keyboard moves send the same event).
    mouse_chose: bool,
    view: WeakEntity<FilePaneView>,
}

impl PaneDelegate {
    fn new(pane: PaneId, handle: AppHandle, view: WeakEntity<FilePaneView>) -> Self {
        Self {
            pane,
            handle,
            entries: Arc::from(Vec::new()),
            has_parent: false,
            sort: Sort::default(),
            selected: BTreeSet::new(),
            anchor: None,
            mouse_chose: false,
            view,
        }
    }

    /// The entry on table row `row` (`None` for the `..` row).
    pub fn entry(&self, row: usize) -> Option<&Entry> {
        row.checked_sub(usize::from(self.has_parent))
            .and_then(|i| self.entries.get(i))
    }

    fn rows(&self) -> usize {
        self.entries.len() + usize::from(self.has_parent)
    }

    /// Applies a mouse press on `row`: plain selects only it, `secondary` toggles it, `shift`
    /// selects the range from the anchor.
    pub fn click_row(&mut self, row: usize, modifiers: Modifiers) {
        self.mouse_chose = true;
        let Some(name) = self.entry(row).map(|e| e.name.clone()) else {
            self.selected.clear();
            self.anchor = None;
            return;
        };
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(row);
            let (from, to) = (anchor.min(row), anchor.max(row));
            let range: Vec<String> = (from..=to)
                .filter_map(|r| self.entry(r).map(|e| e.name.clone()))
                .collect();
            if !modifiers.secondary() {
                self.selected.clear();
            }
            self.selected.extend(range);
        } else if modifiers.secondary() {
            if !self.selected.remove(&name) {
                self.selected.insert(name);
            }
            self.anchor = Some(row);
        } else {
            self.selected.clear();
            self.selected.insert(name);
            self.anchor = Some(row);
        }
    }

    /// Keyboard moves select the row they land on.
    pub fn move_to(&mut self, row: usize) {
        if std::mem::take(&mut self.mouse_chose) {
            return;
        }
        self.selected.clear();
        if let Some(name) = self.entry(row).map(|e| e.name.clone()) {
            self.selected.insert(name);
        }
        self.anchor = Some(row);
    }

    pub fn select_all(&mut self) {
        self.selected = self.entries.iter().map(|e| e.name.clone()).collect();
    }

    /// New listing: the same directory keeps the names that still exist, another one starts
    /// with nothing selected.
    pub fn replace(&mut self, data: &PaneData, same_directory: bool) {
        self.entries = data.entries.clone();
        self.has_parent = data.has_parent;
        self.sort = data.sort;
        if same_directory {
            let names: BTreeSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
            self.selected.retain(|name| names.contains(name.as_str()));
        } else {
            self.selected.clear();
        }
        self.anchor = None;
    }
}

fn sort_key(col_ix: usize) -> Option<SortKey> {
    match col_ix {
        0 => Some(SortKey::Name),
        1 => Some(SortKey::Size),
        2 => Some(SortKey::Modified),
        _ => None,
    }
}

impl TableDelegate for PaneDelegate {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (name, width) = COLUMNS[col_ix];
        let mut column = Column::new(name.to_lowercase(), name).width(gpui_kit::px(width));
        if col_ix == 1 {
            column = column.text_right();
        }
        match sort_key(col_ix) {
            Some(key) if key == self.sort.key => {
                column = if self.sort.ascending {
                    column.ascending()
                } else {
                    column.descending()
                };
            }
            Some(_) => column = column.sortable(),
            None => {}
        }
        column
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if let Some(key) = sort_key(col_ix) {
            self.handle.send(Command::SetSort {
                pane: self.pane,
                sort: Sort {
                    key,
                    ascending: sort != ColumnSort::Descending,
                },
            });
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entry(row_ix) else {
            // the `..` row
            return match col_ix {
                0 => h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::ArrowUp).small())
                    .child("..")
                    .into_any_element(),
                _ => div().into_any_element(),
            };
        };
        match col_ix {
            0 => {
                let icon = if entry.is_dir() {
                    IconName::Folder
                } else {
                    IconName::File
                };
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(icon)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(entry.name.clone())
                    .into_any_element()
            }
            1 if !entry.is_dir() => div().child(format_size(entry.size)).into_any_element(),
            2 => div().child(format_time(entry.modified)).into_any_element(),
            3 => div()
                .child(format_permissions(entry.permissions, entry.is_dir()))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let selected = self
            .entry(row_ix)
            .is_some_and(|e| self.selected.contains(&e.name));
        let background = cx.theme().table_active;
        div()
            .id(("row", row_ix))
            .when(selected, |row| row.bg(background))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |table, event: &MouseDownEvent, _, cx| {
                    table.delegate_mut().click_row(row_ix, event.modifiers);
                    cx.notify();
                }),
            )
    }

    fn context_menu(
        &mut self,
        _row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let remote = self.pane == PaneId::Remote;
        let item =
            |label: &'static str,
             run: fn(&mut FilePaneView, &mut Window, &mut Context<FilePaneView>)| {
                let view = self.view.clone();
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    view.update(cx, |pane, cx| run(pane, window, cx)).ok();
                })
            };
        let mut menu = menu.item(item(
            if remote { "Download" } else { "Upload" },
            |pane, _, cx| pane.transfer_selection(cx),
        ));
        if remote {
            menu = menu
                .separator()
                .item(item("New folder…", |pane, window, cx| {
                    pane.new_folder(window, cx)
                }))
                .item(item("Rename…", |pane, window, cx| {
                    pane.rename_entry(window, cx)
                }))
                .item(item("Delete", |pane, _, cx| pane.delete_entries(cx)))
                .item(item("Permissions…", |pane, window, cx| {
                    pane.open_permissions(window, cx)
                }));
        }
        menu.separator()
            .item(item("Refresh", |pane, _, cx| pane.refresh(cx)))
    }
}

pub struct FilePaneView {
    pub pane: PaneId,
    model: Entity<AppModel>,
    pub table: Entity<TableState<PaneDelegate>>,
    pub path_input: Entity<InputState>,
    /// `(path, generation)` of the listing the table shows.
    shown: Option<(String, u64)>,
    connected: bool,
    _subscriptions: Vec<Subscription>,
}

impl FilePaneView {
    pub fn new(
        pane: PaneId,
        model: Entity<AppModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let handle = model.read(cx).handle.clone();
        let view = cx.weak_entity();
        let table = cx.new(|cx| TableState::new(PaneDelegate::new(pane, handle, view), window, cx));
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("Path"));
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_in(&model, window, |this, _, window, cx| {
            this.sync(window, cx);
        }));
        subscriptions.push(cx.subscribe_in(
            &table,
            window,
            |this, _, event: &TableEvent, _window, cx| match event {
                TableEvent::DoubleClickedRow(row) => this.open_row(*row, cx),
                TableEvent::SelectRow(row) => {
                    this.table.update(cx, |table, cx| {
                        table.delegate_mut().move_to(*row);
                        cx.notify();
                    });
                }
                _ => {}
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &path_input,
            window,
            |this, input, event: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let path = input.read(cx).value().to_string();
                    if !path.trim().is_empty() {
                        this.model.read(cx).send(Command::Navigate {
                            pane: this.pane,
                            path: path.trim().to_owned(),
                        });
                    }
                }
            },
        ));
        let mut view = Self {
            pane,
            model,
            table,
            path_input,
            shown: None,
            connected: pane == PaneId::Local,
            _subscriptions: subscriptions,
        };
        view.sync(window, cx);
        view
    }

    /// Brings the table in line with the latest snapshot.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.model.read(cx).state.clone();
        let Some(data) = pane_data(&state, self.pane) else {
            self.connected = false;
            self.shown = None;
            cx.notify();
            return;
        };
        self.connected = true;
        let key = (data.path.clone(), data.generation);
        if self.shown.as_ref() != Some(&key) {
            let same_directory = self
                .shown
                .as_ref()
                .is_some_and(|(path, _)| *path == data.path);
            if !same_directory {
                let text = data.path.clone();
                self.path_input
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
            self.table.update(cx, |table, cx| {
                table.delegate_mut().replace(&data, same_directory);
                if !same_directory {
                    table.clear_selection(cx);
                }
                cx.notify();
            });
            self.shown = Some(key);
        } else {
            // the sort order can change without a new listing
            self.table.update(cx, |table, cx| {
                if table.delegate().sort != data.sort {
                    table.delegate_mut().sort = data.sort;
                    table.refresh(cx);
                }
            });
        }
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.model.read(cx).send(Command::Refresh(self.pane));
    }

    fn open_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let entry = self.table.read(cx).delegate().entry(row).cloned();
        let model = self.model.read(cx);
        match entry {
            None => model.send(Command::Up(self.pane)),
            Some(entry) if entry.is_dir() => model.send(Command::Navigate {
                pane: self.pane,
                path: entry.name,
            }),
            // a file is sent to the other side, like FileZilla's double-click
            Some(entry) => self.transfer_names(vec![entry.name], cx),
        }
    }

    /// The names a command applies to: the selection, else the row under the cursor.
    pub fn target_names(&self, cx: &App) -> Vec<String> {
        let table = self.table.read(cx);
        let delegate = table.delegate();
        if !delegate.selected.is_empty() {
            return delegate.selected.iter().cloned().collect();
        }
        table
            .selected_row()
            .and_then(|row| delegate.entry(row))
            .map(|e| vec![e.name.clone()])
            .unwrap_or_default()
    }
}

impl Render for FilePaneView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        if !self.connected {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Not connected: double-click a server")
                .into_any_element();
        }
        let error = self.model.read(cx).state.clone().pipe_error(self.pane);
        v_flex()
            .id(("pane", self.pane as usize))
            .key_context("FilePane")
            .size_full()
            .on_action(cx.listener(|this, _: &OpenRow, _, cx| {
                if let Some(row) = this.table.read(cx).selected_row() {
                    this.open_row(row, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ParentDir, _, cx| {
                this.model.read(cx).send(Command::Up(this.pane));
            }))
            .on_action(cx.listener(|this, _: &SelectAllRows, _, cx| {
                this.table.update(cx, |table, cx| {
                    table.delegate_mut().select_all();
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &RefreshPane, _, cx| this.refresh(cx)))
            .on_action(
                cx.listener(|this, _: &NewRemoteFolder, window, cx| this.new_folder(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RenameEntry, window, cx| this.rename_entry(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteEntries, _, cx| this.delete_entries(cx)))
            .on_action(cx.listener(|this, _: &FocusPath, window, cx| this.focus_path(window, cx)))
            .on_action(
                cx.listener(|this, _: &TransferSelection, _, cx| this.transfer_selection(cx)),
            )
            .child(
                div()
                    .p_1()
                    .border_b_1()
                    .border_color(border)
                    .child(Input::new(&self.path_input).small()),
            )
            .child(
                div()
                    .flex_1()
                    .child(DataTable::new(&self.table).stripe(true)),
            )
            .when_some(error, |pane, message| {
                pane.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(message),
                )
            })
            .into_any_element()
    }
}

trait ErrorOf {
    fn pipe_error(&self, pane: PaneId) -> Option<String>;
}

impl ErrorOf for Arc<AppState> {
    fn pipe_error(&self, pane: PaneId) -> Option<String> {
        pane_data(self, pane).and_then(|d| d.error)
    }
}

impl FilePaneView {
    /// The permissions dialog for the selection (or the row under the cursor).
    pub fn open_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = self.target_names(cx);
        if names.is_empty() {
            return;
        }
        let mode = {
            let table = self.table.read(cx);
            let delegate = table.delegate();
            delegate
                .entries
                .iter()
                .find(|e| e.name == names[0])
                .and_then(|e| e.permissions)
                .unwrap_or(0o644)
        };
        crate::dialogs::permissions::open(self.model.clone(), names, mode, window, cx);
    }
}

impl FilePaneView {
    /// Uploads from the local pane, downloads from the remote one.
    fn transfer_names(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        let command = match self.pane {
            PaneId::Local => Command::Upload { names },
            PaneId::Remote => Command::Download { names },
        };
        self.model.read(cx).send(command);
    }

    /// Sends the selection (or the row under the cursor) to the other side.
    pub fn transfer_selection(&mut self, cx: &mut Context<Self>) {
        let names = self.target_names(cx);
        if names.is_empty() {
            return;
        }
        self.table.update(cx, |table, cx| {
            table.delegate_mut().selected.clear();
            cx.notify();
        });
        self.transfer_names(names, cx);
    }
}

impl FilePaneView {
    /// New remote folder (the local pane has no such operation).
    pub fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane != PaneId::Remote {
            return;
        }
        let model = self.model.clone();
        crate::dialogs::tree_ops::ask_name(window, cx, "New remote folder", "", move |name, cx| {
            model.read(cx).send(Command::Mkdir {
                pane: PaneId::Remote,
                name,
            });
        });
    }

    /// Rename the entry under the cursor (remote only).
    pub fn rename_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane != PaneId::Remote {
            return;
        }
        let names = self.target_names(cx);
        let [from] = names.as_slice() else { return };
        let (model, from) = (self.model.clone(), from.clone());
        crate::dialogs::tree_ops::ask_name(window, cx, "Rename", &from.clone(), move |to, cx| {
            model.read(cx).send(Command::Rename {
                pane: PaneId::Remote,
                from: from.clone(),
                to,
            });
        });
    }

    /// Delete the selection (remote only; the app asks for confirmation).
    pub fn delete_entries(&mut self, cx: &mut Context<Self>) {
        if self.pane != PaneId::Remote {
            return;
        }
        let names = self.target_names(cx);
        if !names.is_empty() {
            self.model.read(cx).send(Command::Delete {
                pane: PaneId::Remote,
                names,
            });
        }
    }

    pub fn focus_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.path_input
            .update(cx, |input, cx| input.focus(window, cx));
    }
}
