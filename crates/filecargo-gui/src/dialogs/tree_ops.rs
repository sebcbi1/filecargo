//! Tree operations that ask a question first: new folder, rename, move, delete, import.

use std::path::PathBuf;

use filecargo_app_core::prelude::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Window, div, px,
};

use super::{confirm, open_form};
use crate::model::AppModel;

/// Why `value` cannot be the name of a folder or site.
pub fn name_problem(value: &str) -> Option<&'static str> {
    let value = value.trim();
    if value.is_empty() {
        Some("The name cannot be empty.")
    } else if value.contains('/') {
        Some("The name cannot contain '/'.")
    } else {
        None
    }
}

/// A dialog with one text field.
pub struct TextPromptView {
    label: &'static str,
    pub input: Entity<InputState>,
    pub error: Option<String>,
}

impl TextPromptView {
    fn new(
        label: &'static str,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = initial.to_owned();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(initial, window, cx);
            state
        });
        super::focus_later(window, cx, &input);
        Self {
            label,
            input,
            error: None,
        }
    }
}

impl Render for TextPromptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_2().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w(px(80.)).text_sm().child(self.label))
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(&self.input).id("text-prompt")),
                ),
        );
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(error.clone())),
            );
        }
        body
    }
}

/// Asks for a name and calls `on_name` with the trimmed text when it is valid.
pub fn ask_name(
    window: &mut Window,
    cx: &mut App,
    title: &'static str,
    initial: &str,
    on_name: impl Fn(String, &mut App) + 'static,
) -> Entity<TextPromptView> {
    let view = cx.new(|cx| TextPromptView::new("Name", initial, window, cx));
    open_form(
        window,
        cx,
        title,
        px(420.),
        view.clone(),
        "OK",
        move |view, cx| {
            let text = view.read(cx).input.read(cx).value().to_string();
            match name_problem(&text) {
                Some(problem) => {
                    view.update(cx, |view, cx| {
                        view.error = Some(problem.to_owned());
                        cx.notify();
                    });
                    false
                }
                None => {
                    on_name(text.trim().to_owned(), cx);
                    true
                }
            }
        },
    );
    view
}

pub fn new_folder(
    model: Entity<AppModel>,
    parent: Option<FolderId>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextPromptView> {
    ask_name(window, cx, "New folder", "", move |name, cx| {
        model
            .read(cx)
            .send(Command::Tree(TreeOp::AddFolder { name, parent }));
    })
}

pub fn rename(
    model: Entity<AppModel>,
    node: NodeId,
    current: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextPromptView> {
    ask_name(window, cx, "Rename", current, move |name, cx| {
        model
            .read(cx)
            .send(Command::Tree(TreeOp::Rename { node, name }));
    })
}

pub fn delete(
    model: Entity<AppModel>,
    node: NodeId,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let what = match node {
        NodeId::Folder(_) => format!("Delete the folder \"{name}\" and everything in it?"),
        NodeId::Site(_) => format!("Delete the site \"{name}\"?"),
    };
    confirm(window, cx, "Delete", what, "Delete", true, move |cx| {
        model.read(cx).send(Command::Tree(TreeOp::Delete { node }));
    });
}

#[derive(Clone)]
pub struct FolderChoice {
    pub index: usize,
    pub label: SharedString,
}

impl SearchableListItem for FolderChoice {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &usize {
        &self.index
    }
}

/// Every folder `node` may move into: the top level and all folders except `node` and what is
/// below it, as `(folder, path label)`.
pub fn move_targets(tree: &ServerTree, node: NodeId) -> Vec<(Option<FolderId>, String)> {
    fn path_of(tree: &ServerTree, id: FolderId) -> String {
        let mut names = Vec::new();
        let mut at = Some(id);
        while let Some(folder) = at.and_then(|i| tree.folder(i)) {
            names.push(folder.name.clone());
            at = folder.parent;
        }
        names.reverse();
        format!("/{}", names.join("/"))
    }
    fn inside(tree: &ServerTree, folder: FolderId, root: FolderId) -> bool {
        let mut at = Some(folder);
        while let Some(id) = at {
            if id == root {
                return true;
            }
            at = tree.folder(id).and_then(|f| f.parent);
        }
        false
    }
    let mut targets = vec![(None, "/ (top level)".to_owned())];
    let mut folders: Vec<(Option<FolderId>, String)> = tree
        .folders()
        .iter()
        .filter(|f| !matches!(node, NodeId::Folder(root) if inside(tree, f.id, root)))
        .map(|f| (Some(f.id), path_of(tree, f.id)))
        .collect();
    folders.sort_by(|a, b| a.1.cmp(&b.1));
    targets.extend(folders);
    targets
}

pub struct MoveView {
    pub select: Entity<SelectState<SearchableVec<FolderChoice>>>,
    targets: Vec<(Option<FolderId>, String)>,
}

impl MoveView {
    pub fn chosen(&self, cx: &App) -> Option<Option<FolderId>> {
        let index = *self.select.read(cx).selected_value()?;
        self.targets.get(index).map(|t| t.0)
    }
}

impl Render for MoveView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(80.)).text_sm().child("Folder"))
            .child(
                div()
                    .flex_1()
                    .child(Select::new(&self.select).id("move-target")),
            )
    }
}

pub fn move_to(
    model: Entity<AppModel>,
    node: NodeId,
    window: &mut Window,
    cx: &mut App,
) -> Entity<MoveView> {
    let tree = model.read(cx).state.servers.clone();
    let targets = move_targets(&tree, node);
    let current = match node {
        NodeId::Site(id) => tree.site(id).and_then(|s| s.folder),
        NodeId::Folder(id) => tree.folder(id).and_then(|f| f.parent),
    };
    let start = targets.iter().position(|t| t.0 == current).unwrap_or(0);
    let items: Vec<FolderChoice> = targets
        .iter()
        .enumerate()
        .map(|(index, t)| FolderChoice {
            index,
            label: t.1.clone().into(),
        })
        .collect();
    let view = cx.new(|cx| {
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(items),
                Some(IndexPath::new(start)),
                window,
                cx,
            )
        });
        MoveView { select, targets }
    });
    open_form(
        window,
        cx,
        "Move to",
        px(420.),
        view.clone(),
        "Move",
        move |view, cx| {
            let Some(parent) = view.read(cx).chosen(cx) else {
                return false;
            };
            model
                .read(cx)
                .send(Command::Tree(TreeOp::Move { node, parent }));
            true
        },
    );
    view
}

/// The import dialog: a path (with a picker) and whether to import the saved passwords.
pub struct ImportView {
    pub path: Entity<InputState>,
    passwords: bool,
    pub error: Option<String>,
}

impl ImportView {
    pub fn values(&self, cx: &App) -> (PathBuf, bool) {
        (
            PathBuf::from(self.path.read(cx).value().trim()),
            self.passwords,
        )
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose sitemanager.xml".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    let text = path.display().to_string();
                    this.path
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                })
                .ok();
            }
        })
        .detach();
    }
}

impl Render for ImportView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().w(px(80.)).text_sm().child("File"))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.path).id("import-path")),
                    )
                    .child(
                        Button::new("import-browse")
                            .small()
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            )
            .child(
                Checkbox::new("import-passwords")
                    .label("Import the saved passwords")
                    .checked(self.passwords)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.passwords = *checked;
                        cx.notify();
                    })),
            );
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(error.clone())),
            );
        }
        body
    }
}

/// FileZilla keeps its sites in `~/.config/filezilla/sitemanager.xml` (Linux); other systems
/// differ, so this is only the field's starting text.
fn default_import_path() -> String {
    std::env::home_dir()
        .map(|home| {
            home.join(".config/filezilla/sitemanager.xml")
                .display()
                .to_string()
        })
        .unwrap_or_default()
}

pub fn import(model: Entity<AppModel>, window: &mut Window, cx: &mut App) -> Entity<ImportView> {
    let initial = default_import_path();
    let view = cx.new(|cx| {
        let path = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(initial, window, cx);
            state
        });
        super::focus_later(window, cx, &path);
        ImportView {
            path,
            passwords: false,
            error: None,
        }
    });
    open_form(
        window,
        cx,
        "Import from FileZilla",
        px(560.),
        view.clone(),
        "Import",
        move |view, cx| {
            let (path, import_passwords) = view.read(cx).values(cx);
            if path.as_os_str().is_empty() {
                view.update(cx, |view, cx| {
                    view.error = Some("Choose the file to import.".to_owned());
                    cx.notify();
                });
                return false;
            }
            model.read(cx).send(Command::ImportFileZilla {
                path,
                import_passwords,
            });
            true
        },
    );
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert_eq!(name_problem("  "), Some("The name cannot be empty."));
        assert_eq!(name_problem("a/b"), Some("The name cannot contain '/'."));
        assert_eq!(name_problem(" ok "), None);
    }
}
