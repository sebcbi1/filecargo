//! The app's prompts as dialogs: one per `PromptKind`, each answering through
//! `Command::Answer`. The dialog opens when a new prompt id shows up in the snapshot; closing
//! it any way other than a button (Esc) gives the prompt's "no" answer, so a prompt can never
//! be left unanswered with its dialog gone.

use std::cell::Cell;
use std::rc::Rc;

use filecargo_app_core::prelude::*;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};

use crate::format::{format_size, format_time};
use crate::model::AppModel;

/// Opens a dialog for every new prompt the app shows.
pub struct PromptHost {
    model: Entity<AppModel>,
    shown: Option<PromptId>,
    _subscription: Subscription,
}

impl PromptHost {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe_in(&model, window, |this, _, window, cx| {
            this.sync(window, cx);
        });
        let host = Self {
            model,
            shown: None,
            _subscription,
        };
        // the window is not complete yet (its Root is built after this view): wait a tick
        cx.defer_in(window, |host, window, cx| host.sync(window, cx));
        host
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.model.read(cx).state.prompt.clone();
        match prompt {
            Some(prompt) if self.shown != Some(prompt.id) => {
                self.shown = Some(prompt.id);
                open(self.model.clone(), prompt, window, cx);
            }
            Some(_) => {}
            None => self.shown = None,
        }
    }
}

impl Render for PromptHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Sends the answer once, whichever way the dialog ends.
#[derive(Clone)]
struct Answerer {
    model: Entity<AppModel>,
    id: PromptId,
    done: Rc<Cell<bool>>,
}

impl Answerer {
    fn send(&self, answer: PromptAnswer, cx: &mut App) {
        if !self.done.replace(true) {
            self.model.read(cx).send(Command::Answer {
                id: self.id,
                answer,
            });
        }
    }
}

struct Choice {
    id: &'static str,
    label: &'static str,
    variant: ButtonVariant,
    run: Rc<dyn Fn(&mut App)>,
}

fn choice(
    id: &'static str,
    label: &'static str,
    variant: ButtonVariant,
    run: impl Fn(&mut App) + 'static,
) -> Choice {
    Choice {
        id,
        label,
        variant,
        run: Rc::new(run),
    }
}

type Body = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A dialog with `body` and one button per choice; Esc runs `dismiss`.
fn open_choices(
    window: &mut Window,
    cx: &mut App,
    title: SharedString,
    body: Body,
    choices: Vec<Choice>,
    dismiss: Rc<dyn Fn(&mut App)>,
) {
    let choices = Rc::new(choices);
    window.open_dialog(cx, move |dialog, window, cx| {
        let mut footer = DialogFooter::new();
        for choice in choices.iter() {
            let run = choice.run.clone();
            footer = footer.child(
                Button::new(choice.id)
                    .with_variant(choice.variant)
                    .label(choice.label)
                    .on_click(move |_, window, cx| {
                        run(cx);
                        window.close_dialog(cx);
                    }),
            );
        }
        let dismiss = dismiss.clone();
        dialog
            .title(title.clone())
            .w(px(520.))
            .close_button(false)
            .overlay_closable(false)
            .child(body(window, cx))
            .footer(footer)
            .on_cancel(move |_, _, cx| {
                dismiss(cx);
                true
            })
    });
}

fn text_body(lines: Vec<String>) -> Body {
    Rc::new(move |_, _| {
        let mut body = v_flex().gap_1();
        for line in &lines {
            body = body.child(SharedString::from(line.clone()));
        }
        body.into_any_element()
    })
}

pub fn open(model: Entity<AppModel>, prompt: Prompt, window: &mut Window, cx: &mut App) {
    let answerer = Answerer {
        model: model.clone(),
        id: prompt.id,
        done: Rc::new(Cell::new(false)),
    };
    match prompt.kind {
        PromptKind::Credential(request) => open_credential(answerer, request, window, cx),
        PromptKind::HostKey(request) => {
            let lines = vec![
                format!(
                    "The authenticity of {}:{} cannot be established.",
                    request.host, request.port
                ),
                format!("{} key fingerprint:", request.algorithm),
                request.fingerprint,
            ];
            open_trust(answerer, "Unknown host key", lines, window, cx);
        }
        PromptKind::Certificate(request) => {
            let lines = vec![
                format!("{}:{}: {}.", request.host, request.port, request.problem),
                format!("Subject: {}", request.subject),
                format!("Issuer: {}", request.issuer),
                format!("Expires: {}", request.not_after),
                format!("SHA-256: {}", request.sha256),
            ];
            open_trust(answerer, "Untrusted certificate", lines, window, cx);
        }
        PromptKind::Conflict { conflict, .. } => open_conflict(answerer, conflict, window, cx),
        PromptKind::ConfirmDelete {
            names, recursive, ..
        } => {
            let what = match names.as_slice() {
                [one] => format!("\"{one}\""),
                many => format!("{} items", many.len()),
            };
            let mut lines = vec![format!("Delete {what}?")];
            if recursive {
                lines.push("Folders are deleted with everything in them.".to_owned());
            }
            open_confirm(answerer, "Delete", lines, "Delete", true, window, cx);
        }
        PromptKind::ConfirmQuit { active_transfers } => {
            let lines = vec![format!(
                "{active_transfers} transfer{} still running. Quit anyway?",
                if active_transfers == 1 {
                    " is"
                } else {
                    "s are"
                }
            )];
            open_confirm(answerer, "Quit", lines, "Quit", true, window, cx);
        }
        PromptKind::Message { title, body, .. } => {
            let lines = body.lines().map(str::to_owned).collect();
            let ok = answerer.clone();
            let dismiss = answerer;
            open_choices(
                window,
                cx,
                title.into(),
                text_body(lines),
                vec![choice("ok", "OK", ButtonVariant::Primary, move |cx| {
                    ok.send(PromptAnswer::Dismiss, cx);
                })],
                Rc::new(move |cx| dismiss.send(PromptAnswer::Dismiss, cx)),
            );
        }
    }
}

fn open_confirm(
    answerer: Answerer,
    title: &'static str,
    lines: Vec<String>,
    ok_label: &'static str,
    danger: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let (yes, no, esc) = (answerer.clone(), answerer.clone(), answerer);
    open_choices(
        window,
        cx,
        title.into(),
        text_body(lines),
        vec![
            choice("cancel", "Cancel", ButtonVariant::Default, move |cx| {
                no.send(PromptAnswer::Confirm(false), cx);
            }),
            choice(
                "ok",
                ok_label,
                if danger {
                    ButtonVariant::Danger
                } else {
                    ButtonVariant::Primary
                },
                move |cx| yes.send(PromptAnswer::Confirm(true), cx),
            ),
        ],
        Rc::new(move |cx| esc.send(PromptAnswer::Confirm(false), cx)),
    );
}

fn open_trust(
    answerer: Answerer,
    title: &'static str,
    lines: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let (once, always, reject, esc) = (
        answerer.clone(),
        answerer.clone(),
        answerer.clone(),
        answerer,
    );
    open_choices(
        window,
        cx,
        title.into(),
        text_body(lines),
        vec![
            choice("reject", "Reject", ButtonVariant::Danger, move |cx| {
                reject.send(PromptAnswer::Trust(TrustDecision::Reject), cx);
            }),
            choice("once", "Trust once", ButtonVariant::Default, move |cx| {
                once.send(PromptAnswer::Trust(TrustDecision::TrustOnce), cx);
            }),
            choice(
                "always",
                "Trust always",
                ButtonVariant::Primary,
                move |cx| {
                    always.send(PromptAnswer::Trust(TrustDecision::TrustAlways), cx);
                },
            ),
        ],
        Rc::new(move |cx| esc.send(PromptAnswer::Trust(TrustDecision::Reject), cx)),
    );
}

/// "Apply to all" lives in a tiny entity so the checkbox can re-render the dialog.
pub struct ConflictView {
    lines: Vec<String>,
    pub apply_all: bool,
}

impl Render for ConflictView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_1();
        for line in &self.lines {
            body = body.child(SharedString::from(line.clone()));
        }
        body.child(
            div().pt_2().child(
                Checkbox::new("apply-all")
                    .label("Apply to all remaining files")
                    .checked(self.apply_all)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.apply_all = *checked;
                        cx.notify();
                    })),
            ),
        )
    }
}

fn describe(entry: &Entry) -> String {
    format!(
        "{}  {}",
        format_size(entry.size),
        format_time(entry.modified)
    )
}

fn open_conflict(answerer: Answerer, conflict: ConflictInfo, window: &mut Window, cx: &mut App) {
    let view = cx.new(|_| ConflictView {
        lines: vec![
            format!("\"{}\" already exists.", conflict.source.name),
            format!("New:       {}", describe(&conflict.source)),
            format!("Existing:  {}", describe(&conflict.target)),
        ],
        apply_all: false,
    });
    let rules: [(&'static str, &'static str, ConflictRule, ButtonVariant); 5] = [
        ("skip", "Skip", ConflictRule::Skip, ButtonVariant::Default),
        (
            "rename",
            "Keep both",
            ConflictRule::Rename,
            ButtonVariant::Default,
        ),
        (
            "resume",
            "Resume",
            ConflictRule::Resume,
            ButtonVariant::Default,
        ),
        (
            "newer",
            "Overwrite if newer",
            ConflictRule::OverwriteIfNewer,
            ButtonVariant::Default,
        ),
        (
            "overwrite",
            "Overwrite",
            ConflictRule::Overwrite,
            ButtonVariant::Primary,
        ),
    ];
    let choices = rules
        .into_iter()
        .map(|(id, label, rule, variant)| {
            let answerer = answerer.clone();
            let view = view.clone();
            choice(id, label, variant, move |cx| {
                let apply_to_all = view.read(cx).apply_all;
                answerer.send(
                    PromptAnswer::Conflict(ConflictDecision { rule, apply_to_all }),
                    cx,
                );
            })
        })
        .collect();
    let esc = answerer;
    let body_view = view;
    open_choices(
        window,
        cx,
        "File exists".into(),
        Rc::new(move |_, _| body_view.clone().into_any_element()),
        choices,
        Rc::new(move |cx| esc.send(PromptAnswer::Dismiss, cx)),
    );
}

/// The fields of a credential prompt.
pub struct CredentialView {
    lines: Vec<String>,
    pub inputs: Vec<Entity<InputState>>,
    labels: Vec<String>,
    remember_allowed: bool,
    pub remember: bool,
}

impl CredentialView {
    pub fn answer(&self, cx: &App) -> CredentialAnswer {
        CredentialAnswer {
            values: self
                .inputs
                .iter()
                .map(|input| SecretString::from(input.read(cx).unmask_value().to_string()))
                .collect(),
            remember: self.remember && self.remember_allowed,
        }
    }
}

impl Render for CredentialView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_2();
        for line in &self.lines {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(line.clone())),
            );
        }
        for (index, input) in self.inputs.iter().enumerate() {
            body = body.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(120.))
                            .text_sm()
                            .child(SharedString::from(self.labels[index].clone())),
                    )
                    .child(div().flex_1().child(
                        Input::new(input).id(SharedString::from(format!("credential-{index}"))),
                    )),
            );
        }
        if self.remember_allowed {
            body = body.child(
                Checkbox::new("remember")
                    .label("Remember")
                    .checked(self.remember)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.remember = *checked;
                        cx.notify();
                    })),
            );
        }
        body
    }
}

fn open_credential(
    answerer: Answerer,
    request: CredentialPrompt,
    window: &mut Window,
    cx: &mut App,
) {
    let (title, lines, fields, remember_allowed): (String, Vec<String>, Vec<(String, bool)>, bool) =
        match &request {
            CredentialPrompt::Password { site, user, retry } => (
                format!("Password for {user}@{site}"),
                retry
                    .then(|| "Login failed, try again.".to_owned())
                    .into_iter()
                    .collect(),
                vec![("Password".to_owned(), true)],
                true,
            ),
            CredentialPrompt::Passphrase {
                site,
                key_path,
                retry,
            } => {
                let mut lines = vec![key_path.display().to_string()];
                if *retry {
                    lines.push("Wrong passphrase, try again.".to_owned());
                }
                (
                    format!("Key passphrase ({site})"),
                    lines,
                    vec![("Passphrase".to_owned(), true)],
                    true,
                )
            }
            CredentialPrompt::KeyboardInteractive {
                site,
                name,
                instructions,
                prompts,
            } => (
                if name.is_empty() {
                    site.clone()
                } else {
                    name.clone()
                },
                instructions.lines().map(str::to_owned).collect(),
                prompts
                    .iter()
                    .map(|(text, echo)| (text.trim().trim_end_matches(':').to_owned(), !echo))
                    .collect(),
                false,
            ),
        };
    let inputs: Vec<Entity<InputState>> = fields
        .iter()
        .map(|(_, masked)| {
            let masked = *masked;
            cx.new(|cx| InputState::new(window, cx).masked(masked))
        })
        .collect();
    if let Some(first) = inputs.first() {
        crate::dialogs::focus_later(window, cx, first);
    }
    let view = cx.new(|_| CredentialView {
        lines,
        inputs,
        labels: fields.into_iter().map(|f| f.0).collect(),
        remember_allowed,
        remember: false,
    });
    let (ok, cancel, esc) = (answerer.clone(), answerer.clone(), answerer);
    let for_ok = view.clone();
    let body_view = view;
    open_choices(
        window,
        cx,
        title.into(),
        Rc::new(move |_, _| body_view.clone().into_any_element()),
        vec![
            choice("cancel", "Cancel", ButtonVariant::Default, move |cx| {
                cancel.send(PromptAnswer::Credential(None), cx);
            }),
            choice("ok", "OK", ButtonVariant::Primary, move |cx| {
                let answer = for_ok.read(cx).answer(cx);
                ok.send(PromptAnswer::Credential(Some(answer)), cx);
            }),
        ],
        Rc::new(move |cx| esc.send(PromptAnswer::Credential(None), cx)),
    );
}
