//! The window's content: toolbar, three resizable columns and the bottom panel.

use filecargo_app_core::prelude::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, h_resizable,
    resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, px,
};

use crate::model::AppModel;
use crate::pane::FilePaneView;
use crate::toolbar::session_label;
use crate::tree::ServerTreeView;

gpui_kit::actions!(filecargo, [Quit]);

pub struct Workspace {
    pub model: Entity<AppModel>,
    pub tree: Entity<ServerTreeView>,
    pub local: Entity<FilePaneView>,
    pub remote: Entity<FilePaneView>,
    /// How many times the view drew (tests use it to see that a snapshot re-rendered).
    pub renders: usize,
    _observe: Subscription,
    _observe_tree: Subscription,
}

impl Workspace {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| ServerTreeView::new(model.clone(), cx));
        let local = cx.new(|cx| FilePaneView::new(PaneId::Local, model.clone(), window, cx));
        let remote = cx.new(|cx| FilePaneView::new(PaneId::Remote, model.clone(), window, cx));
        // every new snapshot re-renders the workspace
        let _observe = cx.observe(&model, |_, _, cx| cx.notify());
        // the toolbar depends on the tree selection
        let _observe_tree = cx.observe(&tree, |_, _, cx| cx.notify());
        Self {
            model,
            tree,
            local,
            remote,
            renders: 0,
            _observe,
            _observe_tree,
        }
    }

    fn area(title: &str, cx: &Context<Self>) -> gpui_kit::Div {
        v_flex()
            .size_full()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_owned()),
            )
    }

    /// The site the buttons act on: the one selected in the tree.
    fn selected_site(&self, cx: &gpui_kit::App) -> Option<SiteId> {
        match self.tree.read(cx).selected {
            Some(NodeId::Site(site)) => Some(site),
            _ => None,
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let state = self.model.read(cx).state.clone();
        let connected = matches!(state.session, SessionState::Connected { .. });
        let busy = matches!(state.session, SessionState::Connecting { .. });
        let can_connect = self.selected_site(cx).is_some() && !busy;
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("connect")
                    .small()
                    .icon(IconName::Play)
                    .label("Connect")
                    .disabled(!can_connect)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(site) = this.selected_site(cx) {
                            this.model.read(cx).send(Command::Connect(site));
                        }
                    })),
            )
            .child(
                Button::new("disconnect")
                    .small()
                    .icon(IconName::Close)
                    .label("Disconnect")
                    .disabled(!connected && !busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.read(cx).send(Command::Disconnect);
                    })),
            )
            .child(
                Button::new("refresh")
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let model = this.model.read(cx);
                        model.send(Command::Refresh(PaneId::Local));
                        if model.state.remote.is_some() {
                            model.send(Command::Refresh(PaneId::Remote));
                        }
                    })),
            )
            .child(
                Button::new("new-site")
                    .small()
                    .icon(IconName::Plus)
                    .label("New site")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let folder = this.tree.read(cx).target_folder(cx);
                        crate::dialogs::site_editor::open(
                            this.model.clone(),
                            None,
                            folder,
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                Button::new("new-folder")
                    .small()
                    .icon(IconName::FolderClosed)
                    .label("New folder")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let parent = this.tree.read(cx).target_folder(cx);
                        crate::dialogs::tree_ops::new_folder(
                            this.model.clone(),
                            parent,
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                Button::new("import")
                    .small()
                    .label("Import…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        crate::dialogs::tree_ops::import(this.model.clone(), window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(session_label(&state)),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let toolbar = self.toolbar(cx);
        let tree = v_flex()
            .size_full()
            .border_1()
            .border_color(cx.theme().border)
            .child(self.tree.clone());
        let local = self.local.clone();
        let remote = self.remote.clone();
        let bottom = Self::area("Queue", cx);
        let columns = h_resizable("columns")
            .child(
                resizable_panel()
                    .size(px(220.))
                    .size_range(px(160.)..px(480.))
                    .child(tree),
            )
            .child(resizable_panel().child(local))
            .child(resizable_panel().child(remote));
        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(toolbar)
            .child(
                v_resizable("body")
                    .child(resizable_panel().child(columns))
                    .child(
                        resizable_panel()
                            .size(px(220.))
                            .size_range(px(80.)..px(600.))
                            .child(bottom),
                    ),
            )
    }
}

/// The key bindings of the whole application.
pub fn bind_keys(cx: &mut gpui_kit::App) {
    use crate::pane::{OpenRow, ParentDir, RefreshPane, SelectAllRows};
    use crate::tree::{DeleteSelected, EditSelected, RenameSelected};
    use gpui_kit::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("enter", OpenRow, Some("FilePane")),
        KeyBinding::new("backspace", ParentDir, Some("FilePane")),
        KeyBinding::new("secondary-a", SelectAllRows, Some("FilePane")),
        KeyBinding::new("secondary-r", RefreshPane, Some("FilePane")),
        KeyBinding::new("f2", RenameSelected, Some("ServerTree")),
        KeyBinding::new("delete", DeleteSelected, Some("ServerTree")),
        KeyBinding::new("secondary-e", EditSelected, Some("ServerTree")),
    ]);
}
