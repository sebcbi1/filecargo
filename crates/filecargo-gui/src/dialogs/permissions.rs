//! The permissions dialog: an octal field and the nine rwx boxes, kept in step.

use filecargo_app_core::prelude::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, px,
};

use super::open_form;
use crate::model::AppModel;

pub const BITS: [(&str, u32); 9] = [
    ("Owner read", 0o400),
    ("Owner write", 0o200),
    ("Owner execute", 0o100),
    ("Group read", 0o040),
    ("Group write", 0o020),
    ("Group execute", 0o010),
    ("Others read", 0o004),
    ("Others write", 0o002),
    ("Others execute", 0o001),
];

/// `640`, `0755` or `1777` as a mode; `None` for anything else.
pub fn parse_octal(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || text.len() > 4 {
        return None;
    }
    u32::from_str_radix(text, 8).ok().filter(|m| *m <= 0o7777)
}

pub struct PermissionsView {
    model: Entity<AppModel>,
    names: Vec<String>,
    pub octal: Entity<InputState>,
    /// The mode the boxes show (permission bits plus any special bits typed in octal).
    pub mode: u32,
    pub error: Option<String>,
    _subscription: Subscription,
}

impl PermissionsView {
    fn new(
        model: Entity<AppModel>,
        names: Vec<String>,
        mode: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let octal = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(format!("{:o}", mode & 0o7777), window, cx);
            state
        });
        // typing octal moves the boxes (set_value does not emit Change, so there is no loop)
        let _subscription = cx.subscribe(&octal, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(mode) = parse_octal(&input.read(cx).value())
            {
                this.mode = mode;
                this.error = None;
                cx.notify();
            }
        });
        Self {
            model,
            names,
            octal,
            mode: mode & 0o7777,
            error: None,
            _subscription,
        }
    }

    /// A box was toggled: rewrite the octal text.
    fn toggle(&mut self, bit: u32, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = if on {
            self.mode | bit
        } else {
            self.mode & !bit
        };
        let text = format!("{:o}", self.mode);
        self.octal
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.error = None;
        cx.notify();
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        match parse_octal(&self.octal.read(cx).value()) {
            Some(mode) => {
                self.model.read(cx).send(Command::Chmod {
                    names: self.names.clone(),
                    mode,
                });
                true
            }
            None => {
                self.error = Some("Enter the mode as 3 or 4 octal digits.".to_owned());
                cx.notify();
                false
            }
        }
    }
}

impl Render for PermissionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut grid = h_flex().gap_4().items_start();
        for group in BITS.chunks(3) {
            let mut column = v_flex().gap_1();
            for (label, bit) in group {
                let bit = *bit;
                column = column.child(
                    Checkbox::new(SharedString::from(format!("bit-{bit:o}")))
                        .label(*label)
                        .checked(self.mode & bit != 0)
                        .on_click(cx.listener(move |this, on: &bool, window, cx| {
                            this.toggle(bit, *on, window, cx);
                        })),
                );
            }
            grid = grid.child(column);
        }
        let mut body = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(60.)).text_sm().child("Octal"))
                    .child(div().w(px(100.)).child(Input::new(&self.octal).id("octal"))),
            )
            .child(grid);
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

pub fn open(
    model: Entity<AppModel>,
    names: Vec<String>,
    mode: u32,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PermissionsView> {
    let title = match names.as_slice() {
        [one] => format!("Permissions of {one}"),
        many => format!("Permissions of {} items", many.len()),
    };
    let view = cx.new(|cx| PermissionsView::new(model, names, mode, window, cx));
    open_form(
        window,
        cx,
        title,
        px(520.),
        view.clone(),
        "Apply",
        |view, cx| view.update(cx, |view, cx| view.submit(cx)),
    );
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_text_is_parsed_strictly() {
        assert_eq!(parse_octal("644"), Some(0o644));
        assert_eq!(parse_octal(" 1777 "), Some(0o1777));
        assert_eq!(parse_octal("0755"), Some(0o755));
        for bad in ["", "9", "12345", "-1", "rwx"] {
            assert_eq!(parse_octal(bad), None, "{bad}");
        }
    }
}
