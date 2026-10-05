//! Dialogs: each one is a small view entity that owns its inputs (the dialog builder re-runs
//! on every frame, so state cannot live inside it) plus a function that opens it.

pub mod permissions;
pub mod settings;
pub mod site_editor;
pub mod site_form;
pub mod tree_ops;

use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::{App, Entity, ParentElement as _, Pixels, Render, SharedString, Window};

/// Opens a dialog whose body is the entity `content`, with Cancel and an OK button. `on_ok`
/// returns whether the dialog may close (false keeps it open, e.g. after a validation error).
pub fn open_form<V: Render + 'static>(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    width: Pixels,
    content: Entity<V>,
    ok_label: &'static str,
    on_ok: impl Fn(&Entity<V>, &mut App) -> bool + 'static,
) {
    let title: SharedString = title.into();
    let on_ok = Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _, _| {
        let content_for_ok = content.clone();
        let on_ok = on_ok.clone();
        dialog
            .title(title.clone())
            .w(width)
            .child(content.clone())
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label(ok_label))),
            )
            .on_ok(move |_, _, cx| on_ok(&content_for_ok, cx))
    });
}

/// A yes / no question; `on_ok` runs when it is confirmed. `danger` paints the button red.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    ok_label: &'static str,
    danger: bool,
    on_ok: impl Fn(&mut App) + 'static,
) {
    use gpui_kit::component::button::ButtonVariant;
    let (title, description): (SharedString, SharedString) = (title.into(), description.into());
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        let alert = alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok_label)
            .on_ok(move |_, _, cx| {
                on_ok(cx);
                true
            });
        if danger {
            alert.ok_variant(ButtonVariant::Danger)
        } else {
            alert
        }
    });
}

/// Puts the keyboard focus in `input` once the dialog around it is on screen.
pub fn focus_later(
    window: &mut Window,
    cx: &mut App,
    input: &Entity<gpui_kit::component::input::InputState>,
) {
    let input = input.clone();
    window.defer(cx, move |window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
    });
}
