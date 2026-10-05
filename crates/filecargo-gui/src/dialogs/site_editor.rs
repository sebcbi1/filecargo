//! The site editor: new site and edit site.

use filecargo_app_core::prelude::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Window, div, px,
};

use super::open_form;
use super::site_form::{AUTHS, FTP_MODES, FormValues, PROTOCOLS};
use crate::model::AppModel;

type Choice = Entity<SelectState<SearchableVec<&'static str>>>;

pub struct SiteEditorView {
    model: Entity<AppModel>,
    original: Option<Site>,
    folder: Option<FolderId>,
    pub name: Entity<InputState>,
    pub host: Entity<InputState>,
    pub port: Entity<InputState>,
    pub user: Entity<InputState>,
    pub password: Entity<InputState>,
    pub key_path: Entity<InputState>,
    pub remote_dir: Entity<InputState>,
    pub local_dir: Entity<InputState>,
    pub notes: Entity<InputState>,
    pub protocol: Choice,
    pub auth: Choice,
    pub ftp_mode: Choice,
    remember: bool,
    pub error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

fn text(
    window: &mut Window,
    cx: &mut Context<SiteEditorView>,
    placeholder: &'static str,
    value: &str,
) -> Entity<InputState> {
    let value = value.to_owned();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        state.set_value(value, window, cx);
        state
    })
}

fn choice(
    window: &mut Window,
    cx: &mut Context<SiteEditorView>,
    items: &[&'static str],
    selected: usize,
) -> Choice {
    let items = items.to_vec();
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(items),
            Some(IndexPath::new(selected)),
            window,
            cx,
        )
    })
}

impl SiteEditorView {
    pub fn new(
        model: Entity<AppModel>,
        original: Option<Site>,
        folder: Option<FolderId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let values = original.as_ref().map(FormValues::of).unwrap_or_default();
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let protocol = choice(window, cx, &PROTOCOLS, values.protocol);
        let auth = choice(window, cx, &AUTHS, values.auth);
        let ftp_mode = choice(window, cx, &FTP_MODES, usize::from(values.ftp_active));
        let mut subscriptions = Vec::new();
        for select in [&protocol, &auth] {
            // the visible fields depend on these
            subscriptions.push(cx.observe(select, |_, _, cx| cx.notify()));
        }
        Self {
            model,
            original,
            folder,
            name: text(window, cx, "Name", &values.name),
            host: text(window, cx, "Host", &values.host),
            port: text(window, cx, "Default for the protocol", &values.port),
            user: text(window, cx, "User", &values.user),
            password,
            key_path: text(window, cx, "Path of the private key", &values.key_path),
            remote_dir: text(window, cx, "Start folder on the server", &values.remote_dir),
            local_dir: text(
                window,
                cx,
                "Start folder on this computer",
                &values.local_dir,
            ),
            notes: text(window, cx, "Notes", &values.notes),
            protocol,
            auth,
            ftp_mode,
            remember: values.remember,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn index_of(select: &Choice, items: &[&'static str], cx: &App) -> usize {
        select
            .read(cx)
            .selected_value()
            .and_then(|value| items.iter().position(|item| item == value))
            .unwrap_or(0)
    }

    pub fn values(&self, cx: &App) -> FormValues {
        let read = |state: &Entity<InputState>| state.read(cx).value().to_string();
        FormValues {
            name: read(&self.name),
            protocol: Self::index_of(&self.protocol, &PROTOCOLS, cx),
            host: read(&self.host),
            port: read(&self.port),
            user: read(&self.user),
            auth: Self::index_of(&self.auth, &AUTHS, cx),
            password: self.password.read(cx).unmask_value().to_string(),
            key_path: read(&self.key_path),
            remember: self.remember,
            ftp_active: Self::index_of(&self.ftp_mode, &FTP_MODES, cx) == 1,
            remote_dir: read(&self.remote_dir),
            local_dir: read(&self.local_dir),
            notes: read(&self.notes),
        }
    }

    /// Saves the form: sends the commands and returns true, or shows what is wrong and
    /// returns false.
    pub fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        match self
            .values(cx)
            .commands(self.original.as_ref(), self.folder)
        {
            Ok(commands) => {
                for command in commands {
                    self.model.read(cx).send(command);
                }
                true
            }
            Err(message) => {
                self.error = Some(message);
                cx.notify();
                false
            }
        }
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose the private key".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    let text = path.display().to_string();
                    this.key_path
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                })
                .ok();
            }
        })
        .detach();
    }

    fn row(label: &'static str, control: impl IntoElement) -> gpui_kit::Div {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(120.)).text_sm().child(label))
            .child(div().flex_1().child(control))
    }
}

impl Render for SiteEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let values = self.values(cx);
        let ftp = values.protocol != 0;
        let mut form = v_flex()
            .gap_2()
            .child(Self::row("Name", Input::new(&self.name).id("site-name")))
            .child(Self::row(
                "Protocol",
                Select::new(&self.protocol).id("site-protocol"),
            ))
            .child(Self::row("Host", Input::new(&self.host).id("site-host")))
            .child(Self::row("Port", Input::new(&self.port).id("site-port")))
            .child(Self::row("User", Input::new(&self.user).id("site-user")))
            .child(Self::row("Login", Select::new(&self.auth).id("site-auth")));
        match values.auth {
            0 => {
                form = form.child(Self::row(
                    "Password",
                    Input::new(&self.password).id("site-password"),
                ));
            }
            1 => {
                form =
                    form.child(Self::row(
                        "Key file",
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.key_path).id("site-key")),
                            )
                            .child(Button::new("browse").small().label("Browse…").on_click(
                                cx.listener(|this, _, window, cx| this.browse(window, cx)),
                            )),
                    ));
            }
            _ => {}
        }
        if values.auth <= 1 {
            form = form.child(Self::row(
                "",
                Checkbox::new("site-remember")
                    .label(if values.auth == 0 {
                        "Remember the password"
                    } else {
                        "Remember the passphrase"
                    })
                    .checked(self.remember)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.remember = *checked;
                        cx.notify();
                    })),
            ));
        }
        if ftp {
            form = form.child(Self::row(
                "FTP mode",
                Select::new(&self.ftp_mode).id("site-ftp-mode"),
            ));
        }
        form = form
            .child(Self::row(
                "Remote folder",
                Input::new(&self.remote_dir).id("site-remote-dir"),
            ))
            .child(Self::row(
                "Local folder",
                Input::new(&self.local_dir).id("site-local-dir"),
            ))
            .child(Self::row("Notes", Input::new(&self.notes).id("site-notes")));
        if let Some(error) = &self.error {
            form = form.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(error.clone())),
            );
        }
        form
    }
}

/// Opens the editor for a new site in `folder`, or for `original`.
pub fn open(
    model: Entity<AppModel>,
    original: Option<Site>,
    folder: Option<FolderId>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SiteEditorView> {
    let title = if original.is_some() {
        "Edit site"
    } else {
        "New site"
    };
    let view = cx.new(|cx| SiteEditorView::new(model, original, folder, window, cx));
    open_form(
        window,
        cx,
        title,
        px(560.),
        view.clone(),
        "Save",
        |view, cx| view.update(cx, |view, cx| view.submit(cx)),
    );
    view
}

impl SiteEditorView {
    pub fn set_remember(&mut self, remember: bool, cx: &mut Context<Self>) {
        self.remember = remember;
        cx.notify();
    }
}
