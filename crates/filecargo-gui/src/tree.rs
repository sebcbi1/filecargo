//! The server tree: folders and sites from `ServerTree`, with the session state on the rows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use filecargo_app_core::prelude::*;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tree::{Tree, TreeEvent, TreeItem, TreeState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, px,
};

use crate::model::AppModel;

fn item_id(node: NodeId) -> String {
    match node {
        NodeId::Folder(id) => format!("folder:{id}"),
        NodeId::Site(id) => format!("site:{id}"),
    }
}

/// The tree's items (children of `parent`), and the node every item id stands for.
pub fn build_items(
    tree: &ServerTree,
    parent: Option<FolderId>,
    expanded: &HashSet<String>,
    nodes: &mut HashMap<String, NodeId>,
) -> Vec<TreeItem> {
    let mut items = Vec::new();
    for node in tree.children(parent) {
        match node {
            Node::Folder(folder) => {
                let node = NodeId::Folder(folder.id);
                let id = item_id(node);
                nodes.insert(id.clone(), node);
                let children = build_items(tree, Some(folder.id), expanded, nodes);
                items.push(
                    TreeItem::new(id.clone(), folder.name.clone())
                        .expanded(expanded.contains(&id))
                        .children(children),
                );
            }
            Node::Site(site) => {
                let node = NodeId::Site(site.id);
                let id = item_id(node);
                nodes.insert(id.clone(), node);
                items.push(TreeItem::new(id, site.name.clone()));
            }
        }
    }
    items
}

pub struct ServerTreeView {
    model: Entity<AppModel>,
    state: Entity<TreeState>,
    nodes: Arc<HashMap<String, NodeId>>,
    expanded: HashSet<String>,
    shown: Option<Arc<ServerTree>>,
    /// The row last clicked or moved to.
    pub selected: Option<NodeId>,
    _subscriptions: Vec<Subscription>,
}

impl ServerTreeView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TreeState::new(cx));
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe(&model, |this, _, cx| this.sync(cx)));
        subscriptions.push(
            cx.subscribe(&state, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.to_string());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id.as_ref());
                }
            }),
        );
        subscriptions.push(cx.observe(&state, |this, state, cx| {
            let id = state
                .read(cx)
                .selected_item()
                .map(|item| item.id.to_string());
            this.selected = id.and_then(|id| this.nodes.get(&id).copied());
            cx.notify();
        }));
        let mut view = Self {
            model,
            state,
            nodes: Arc::default(),
            expanded: HashSet::new(),
            shown: None,
            selected: None,
            _subscriptions: subscriptions,
        };
        view.sync(cx);
        view
    }

    /// Rebuilds the items when the tree changed, keeping what was open.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let tree = self.model.read(cx).state.servers.clone();
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, &tree))
        {
            cx.notify();
            return;
        }
        let mut nodes = HashMap::new();
        let items = build_items(&tree, None, &self.expanded, &mut nodes);
        // folders that no longer exist are forgotten
        self.expanded.retain(|id| nodes.contains_key(id));
        self.nodes = Arc::new(nodes);
        self.shown = Some(tree);
        self.state
            .update(cx, |state, cx| state.set_items(items, cx));
        if self
            .selected
            .is_some_and(|node| !self.nodes.values().any(|n| *n == node))
        {
            self.selected = None;
        }
        cx.notify();
    }
}

impl Render for ServerTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.model.read(cx).state.session.clone();
        let nodes = self.nodes.clone();
        let model = self.model.clone();
        let menu_model = self.model.clone();
        let menu_nodes = self.nodes.clone();
        let menu_session = session.clone();
        let menu_tree = self.model.read(cx).state.servers.clone();
        let colors = (
            cx.theme().success,
            cx.theme().muted_foreground,
            cx.theme().danger,
        );
        let tree = Tree::new(&self.state, move |ix, entry, selected, _window, _cx| {
            let node = nodes.get(entry.item().id.as_ref()).copied();
            let (icon, color) = match node {
                Some(NodeId::Folder(_)) => (
                    if entry.is_expanded() {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    },
                    colors.1,
                ),
                _ => match (&session, node) {
                    (SessionState::Connected { site, .. }, Some(NodeId::Site(id)))
                        if *site == id =>
                    {
                        (IconName::CircleCheck, colors.0)
                    }
                    (SessionState::Failed { site, .. }, Some(NodeId::Site(id))) if *site == id => {
                        (IconName::CircleX, colors.2)
                    }
                    (SessionState::Connecting { site, .. }, Some(NodeId::Site(id)))
                        if *site == id =>
                    {
                        (IconName::Loader, colors.1)
                    }
                    _ => (IconName::Globe, colors.1),
                },
            };
            let model = model.clone();
            ListItem::new(("node", ix))
                .selected(selected)
                .pl(px(8. + 14. * entry.depth() as f32))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(icon).small().text_color(color))
                        .child(entry.item().label.clone()),
                )
                .on_click(move |event, _window, cx| {
                    if event.click_count() == 2
                        && let Some(NodeId::Site(site)) = node
                    {
                        model.update(cx, |model, _| model.send(Command::Connect(site)));
                    }
                })
        })
        .context_menu(move |_ix, entry, menu: PopupMenu, _window, _cx| {
            let Some(node) = menu_nodes.get(entry.item().id.as_ref()).copied() else {
                return menu;
            };
            node_menu(
                menu,
                node,
                entry.item().label.to_string(),
                &menu_session,
                &menu_model,
                &menu_tree,
            )
        })
        .size_full();
        div()
            .id("server-tree")
            .key_context("ServerTree")
            .size_full()
            .on_action(cx.listener(|this, _: &RenameSelected, window, cx| {
                if let Some((node, name)) = this.selected_info(cx) {
                    crate::dialogs::tree_ops::rename(this.model.clone(), node, &name, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, window, cx| {
                if let Some((node, name)) = this.selected_info(cx) {
                    crate::dialogs::tree_ops::delete(this.model.clone(), node, &name, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditSelected, window, cx| {
                if let Some((NodeId::Site(id), _)) = this.selected_info(cx)
                    && let Some(site) = this.model.read(cx).state.servers.site(id).cloned()
                {
                    crate::dialogs::site_editor::open(
                        this.model.clone(),
                        Some(site),
                        None,
                        window,
                        cx,
                    );
                }
            }))
            .child(tree)
    }
}

type MenuAction = Box<dyn Fn(&mut Window, &mut gpui_kit::App)>;

/// The right-click menu of a row.
fn node_menu(
    menu: PopupMenu,
    node: NodeId,
    name: String,
    session: &SessionState,
    model: &Entity<AppModel>,
    tree: &Arc<ServerTree>,
) -> PopupMenu {
    use crate::dialogs::{site_editor, tree_ops};
    let item = |label: &'static str, run: MenuAction| {
        PopupMenuItem::new(label).on_click(move |_, window, cx| run(window, cx))
    };
    let mut menu = menu;
    match node {
        NodeId::Site(site) => {
            let connected =
                matches!(session, SessionState::Connected { site: s, .. } if *s == site);
            let m = model.clone();
            menu = menu.item(item(
                "Connect",
                Box::new(move |_, cx| m.update(cx, |m, _| m.send(Command::Connect(site)))),
            ));
            if connected {
                let m = model.clone();
                menu = menu.item(item(
                    "Disconnect",
                    Box::new(move |_, cx| m.update(cx, |m, _| m.send(Command::Disconnect))),
                ));
            }
            menu = menu.separator();
            if let Some(original) = tree.site(site).cloned() {
                let m = model.clone();
                menu = menu.item(item(
                    "Edit…",
                    Box::new(move |window, cx| {
                        site_editor::open(m.clone(), Some(original.clone()), None, window, cx);
                    }),
                ));
            }
            let m = model.clone();
            menu = menu.item(item(
                "Duplicate",
                Box::new(move |_, cx| {
                    m.update(cx, |m, _| m.send(Command::Tree(TreeOp::Duplicate { site })))
                }),
            ));
        }
        NodeId::Folder(folder) => {
            let m = model.clone();
            menu = menu.item(item(
                "New site here…",
                Box::new(move |window, cx| {
                    site_editor::open(m.clone(), None, Some(folder), window, cx);
                }),
            ));
            let m = model.clone();
            menu = menu.item(item(
                "New folder here…",
                Box::new(move |window, cx| {
                    tree_ops::new_folder(m.clone(), Some(folder), window, cx);
                }),
            ));
            menu = menu.separator();
        }
    }
    let (m, n) = (model.clone(), name.clone());
    menu = menu.item(item(
        "Rename…",
        Box::new(move |window, cx| {
            tree_ops::rename(m.clone(), node, &n, window, cx);
        }),
    ));
    let m = model.clone();
    menu = menu.item(item(
        "Move to…",
        Box::new(move |window, cx| {
            tree_ops::move_to(m.clone(), node, window, cx);
        }),
    ));
    let m = model.clone();
    menu.separator().item(item(
        "Delete…",
        Box::new(move |window, cx| tree_ops::delete(m.clone(), node, &name, window, cx)),
    ))
}

gpui_kit::actions!(
    filecargo_tree,
    [RenameSelected, DeleteSelected, EditSelected]
);

impl ServerTreeView {
    /// The selected node and its name.
    pub fn selected_info(&self, cx: &gpui_kit::App) -> Option<(NodeId, String)> {
        let tree = self.model.read(cx).state.servers.clone();
        match self.selected? {
            NodeId::Site(id) => tree.site(id).map(|s| (NodeId::Site(id), s.name.clone())),
            NodeId::Folder(id) => tree
                .folder(id)
                .map(|f| (NodeId::Folder(id), f.name.clone())),
        }
    }

    /// Where a new site or folder goes: the selected folder, or the folder of the selected site.
    pub fn target_folder(&self, cx: &gpui_kit::App) -> Option<FolderId> {
        match self.selected? {
            NodeId::Folder(id) => Some(id),
            NodeId::Site(id) => self
                .model
                .read(cx)
                .state
                .servers
                .site(id)
                .and_then(|s| s.folder),
        }
    }
}

impl ServerTreeView {
    /// How many nodes (folders and sites) the tree holds.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}
