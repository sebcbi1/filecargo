//! The window's content: toolbar, three resizable columns and the bottom panel.

use gpui_kit::component::{
    ActiveTheme as _, h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, px,
};

use crate::model::AppModel;
use crate::pane::FilePaneView;
use filecargo_app_core::prelude::PaneId;
use gpui_kit::AppContext as _;

gpui_kit::actions!(filecargo, [Quit]);

pub struct Workspace {
    pub model: Entity<AppModel>,
    pub local: Entity<FilePaneView>,
    pub remote: Entity<FilePaneView>,
    /// How many times the view drew (tests use it to see that a snapshot re-rendered).
    pub renders: usize,
    _observe: Subscription,
}

impl Workspace {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let local = cx.new(|cx| FilePaneView::new(PaneId::Local, model.clone(), window, cx));
        let remote = cx.new(|cx| FilePaneView::new(PaneId::Remote, model.clone(), window, cx));
        // every new snapshot re-renders the workspace
        let _observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            local,
            remote,
            renders: 0,
            _observe,
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
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let sites = self.model.read(cx).state.servers.sites().len();
        let tree = Self::area(&format!("Servers ({sites})"), cx);
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
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child("filecargo"),
            )
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
    use gpui_kit::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("enter", OpenRow, Some("FilePane")),
        KeyBinding::new("backspace", ParentDir, Some("FilePane")),
        KeyBinding::new("secondary-a", SelectAllRows, Some("FilePane")),
        KeyBinding::new("secondary-r", RefreshPane, Some("FilePane")),
    ]);
}
