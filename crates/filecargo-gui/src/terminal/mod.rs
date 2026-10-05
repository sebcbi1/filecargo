//! The terminal tab: a custom element painting the emulator's cells, and the view that turns
//! keys, the mouse and the clipboard into commands.

pub mod keys;
pub mod runs;

use std::cell::Cell as StdCell;
use std::rc::Rc;

use filecargo_app_core::prelude::*;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollWheelEvent, SharedString, Styled as _,
    Subscription, TextAlign, TextRun, Window, canvas, div, fill, font, point, px, rgb, size,
};

use crate::model::AppModel;
use keys::map_keystroke;
use runs::{Rgb, row_layout};

/// Where the grid is on screen and how big a cell is, as painted last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub cols: u16,
    pub rows: u16,
}

impl Metrics {
    /// The `(row, col)` of the cell under `position`, clamped to the grid.
    pub fn cell_at(&self, position: Point<Pixels>) -> (u16, u16) {
        let col = ((position.x - self.origin.x) / self.cell_width)
            .floor()
            .max(0.0) as u16;
        let row = ((position.y - self.origin.y) / self.line_height)
            .floor()
            .max(0.0) as u16;
        (
            row.min(self.rows.saturating_sub(1)),
            col.min(self.cols.saturating_sub(1)),
        )
    }
}

pub const FONT_SIZE: f32 = 13.0;

gpui_kit::actions!(filecargo_terminal, [SendTab, SendBackTab]);

pub struct TerminalView {
    model: Entity<AppModel>,
    focus: FocusHandle,
    metrics: Rc<StdCell<Option<Metrics>>>,
    last_size: Rc<StdCell<Option<(u16, u16)>>>,
    /// `(row, col)` of both ends of the mouse selection.
    pub selection: Option<((u16, u16), (u16, u16))>,
    selecting: bool,
    open_requested: bool,
    _subscription: Subscription,
}

impl TerminalView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            focus: cx.focus_handle(),
            metrics: Rc::default(),
            last_size: Rc::default(),
            selection: None,
            selecting: false,
            open_requested: false,
            _subscription,
        }
    }

    /// Focuses the terminal when it shows a screen (otherwise there is nothing to type into and
    /// the focus stays where it was).
    pub fn focus(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        if self.handle(cx).is_some() {
            self.focus.focus(window, cx);
        }
    }

    fn handle(&self, cx: &gpui_kit::App) -> Option<TerminalHandle> {
        match &self.model.read(cx).state.terminal {
            TerminalState::Open(view) | TerminalState::Exited { view, .. } => {
                Some(view.handle.clone())
            }
            _ => None,
        }
    }

    fn send_bytes(&self, bytes: Vec<u8>, cx: &gpui_kit::App) {
        if !bytes.is_empty() {
            self.model.read(cx).send(Command::TerminalInput(bytes));
        }
    }

    fn selected_text(&self, cx: &gpui_kit::App) -> Option<String> {
        let (from, to) = self.selection?;
        if from == to {
            return None;
        }
        let text = self.handle(cx)?.selection_text(from, (to.0, to.1 + 1));
        (!text.is_empty()).then_some(text)
    }

    /// Sends a key without going through a key-down event (`Tab` is a focus-traversal binding,
    /// so it arrives as an action).
    fn send_key(&mut self, key: Key, mods: Mods, cx: &mut Context<Self>) {
        if let Some(handle) = self.handle(cx) {
            self.send_bytes(handle.encode_key(key, mods), cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let state = self.model.read(cx).state.terminal.clone();
        if let TerminalState::Exited { .. } = state {
            if stroke.key == "enter" {
                let (cols, rows) = self.last_size.get().unwrap_or((80, 24));
                self.model
                    .read(cx)
                    .send(Command::TerminalOpen { cols, rows });
                self.open_requested = true;
            }
            cx.stop_propagation();
            return;
        }
        let Some(handle) = self.handle(cx) else {
            return;
        };
        let mods = stroke.modifiers;
        let copy =
            stroke.key == "c" && (mods.platform || (mods.control && mods.shift) || mods.control);
        let paste = stroke.key == "v" && (mods.platform || (mods.control && mods.shift));
        let mac_or_explicit = mods.platform || (mods.control && mods.shift);
        if copy {
            if let Some(text) = self.selected_text(cx) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.selection = None;
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if mac_or_explicit {
                // nothing selected: Cmd-C / Ctrl-Shift-C never go to the shell
                cx.stop_propagation();
                return;
            }
        }
        let plain_ctrl_v = stroke.key == "v" && mods.control && !mods.shift && !mods.platform;
        if paste || plain_ctrl_v {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.send_bytes(handle.paste_bytes(&text), cx);
                cx.stop_propagation();
                return;
            }
            if mac_or_explicit {
                cx.stop_propagation();
                return;
            }
        }
        if let Some((key, mods)) = map_keystroke(&stroke.key, stroke.key_char.as_deref(), mods) {
            self.send_bytes(handle.encode_key(key, mods), cx);
            self.selection = None;
        }
        cx.stop_propagation();
        let _ = window;
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        if let Some(metrics) = self.metrics.get() {
            let cell = metrics.cell_at(event.position);
            self.selection = Some((cell, cell));
            self.selecting = true;
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting
            && event.pressed_button == Some(MouseButton::Left)
            && let (Some(metrics), Some((from, _))) = (self.metrics.get(), self.selection)
        {
            self.selection = Some((from, metrics.cell_at(event.position)));
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        self.selecting = false;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines = event.delta.pixel_delta(px(FONT_SIZE * 1.2)).y / px(FONT_SIZE * 1.2);
        let lines = lines.round() as i32;
        if lines != 0 {
            self.model.read(cx).send(Command::TerminalScroll(lines));
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn color(rgb_value: Rgb) -> Hsla {
    rgb(rgb_value.hex()).into()
}

fn theme_rgb(color: Hsla) -> Rgb {
    let rgba = color.to_rgb();
    Rgb(
        (rgba.r * 255.0) as u8,
        (rgba.g * 255.0) as u8,
        (rgba.b * 255.0) as u8,
    )
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.model.read(cx).state.terminal.clone();
        let message = |text: &'static str, cx: &mut Context<Self>| {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element()
        };
        let (handle, exited) = match &state {
            TerminalState::NotAvailable => {
                self.open_requested = false;
                return message(
                    "The terminal needs an SFTP connection (FTP has no shell).",
                    cx,
                );
            }
            TerminalState::Closed => {
                if !self.open_requested {
                    self.open_requested = true;
                    let (cols, rows) = self.last_size.get().unwrap_or((100, 24));
                    self.model
                        .read(cx)
                        .send(Command::TerminalOpen { cols, rows });
                }
                return message("Opening the shell…", cx);
            }
            TerminalState::Open(view) => {
                self.open_requested = false;
                (view.handle.clone(), None)
            }
            TerminalState::Exited { code, view } => (view.handle.clone(), Some(*code)),
        };
        let theme = cx.theme().clone();
        let (default_fg, default_bg) = (theme_rgb(theme.foreground), theme_rgb(theme.background));
        let selection_color = theme.selection;
        let caret = theme.caret;
        let family = theme.mono_font_family.clone();
        let focused = self.focus.is_focused(_window);
        let metrics = self.metrics.clone();
        let last_size = self.last_size.clone();
        let app = self.model.read(cx).handle.clone();
        let selection = self.selection;
        let grid = canvas(
            move |bounds: Bounds<Pixels>, window, _cx| {
                let text_system = window.text_system().clone();
                let base = font(family.clone());
                let font_id = text_system.resolve_font(&base);
                let font_size = px(FONT_SIZE);
                let advance = text_system
                    .em_advance(font_id, font_size)
                    .unwrap_or(px(8.0));
                let scale = window.scale_factor();
                let cell_width = px(((advance * scale).round() / scale).into());
                let line_height = px((FONT_SIZE * 1.25).round());
                let cols = ((bounds.size.width / cell_width).floor().max(1.0)) as u16;
                let rows = ((bounds.size.height / line_height).floor().max(1.0)) as u16;
                if last_size.get() != Some((cols, rows)) {
                    last_size.set(Some((cols, rows)));
                    app.send(Command::TerminalResize { cols, rows });
                }
                let grid = Metrics {
                    origin: bounds.origin,
                    cell_width,
                    line_height,
                    cols,
                    rows,
                };
                metrics.set(Some(grid));
                (grid, base)
            },
            move |bounds, (grid, base), window, cx| {
                window.paint_quad(fill(bounds, color(default_bg)));
                let font_size = px(FONT_SIZE);
                handle.with_screen(|screen| {
                    let (screen_rows, screen_cols) = screen.size();
                    let rows = grid.rows.min(screen_rows);
                    let cols = grid.cols.min(screen_cols);
                    for row in 0..rows {
                        let top = grid.origin.y + grid.line_height * f32::from(row);
                        let layout = row_layout(screen, row, cols, default_fg, default_bg);
                        for background in &layout.backgrounds {
                            let at =
                                point(grid.origin.x + grid.cell_width * background.col as f32, top);
                            window.paint_quad(fill(
                                Bounds::new(
                                    at,
                                    size(
                                        grid.cell_width * background.cells as f32,
                                        grid.line_height,
                                    ),
                                ),
                                color(background.color),
                            ));
                        }
                        if let Some((from, to)) = selection {
                            let (start, end) = if from <= to { (from, to) } else { (to, from) };
                            if row >= start.0 && row <= end.0 {
                                let first = if row == start.0 { start.1 } else { 0 };
                                let last = if row == end.0 {
                                    end.1
                                } else {
                                    cols.saturating_sub(1)
                                };
                                if last >= first {
                                    let at = point(
                                        grid.origin.x + grid.cell_width * f32::from(first),
                                        top,
                                    );
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            at,
                                            size(
                                                grid.cell_width * f32::from(last - first + 1),
                                                grid.line_height,
                                            ),
                                        ),
                                        selection_color,
                                    ));
                                }
                            }
                        }
                        for segment in &layout.segments {
                            let runs: Vec<TextRun> = segment
                                .runs
                                .iter()
                                .map(|(len, style)| {
                                    let mut face = base.clone();
                                    if style.bold {
                                        face = face.bold();
                                    }
                                    if style.italic {
                                        face = face.italic();
                                    }
                                    TextRun {
                                        len: *len,
                                        font: face,
                                        color: color(style.fg),
                                        background_color: None,
                                        underline: style.underline.then(|| {
                                            gpui_kit::UnderlineStyle {
                                                thickness: px(1.0),
                                                color: Some(color(style.fg)),
                                                wavy: false,
                                            }
                                        }),
                                        strikethrough: None,
                                    }
                                })
                                .collect();
                            let shaped = window.text_system().shape_line(
                                SharedString::from(segment.text.clone()),
                                font_size,
                                &runs,
                                Some(
                                    grid.cell_width
                                        * (segment.cells as f32
                                            / segment.text.chars().count().max(1) as f32),
                                ),
                            );
                            let origin =
                                point(grid.origin.x + grid.cell_width * segment.col as f32, top);
                            shaped
                                .paint(origin, grid.line_height, TextAlign::Left, None, window, cx)
                                .ok();
                        }
                    }
                    if !screen.hide_cursor() && screen.scrollback() == 0 {
                        let (row, col) = screen.cursor_position();
                        if row < rows && col < cols {
                            let at = point(
                                grid.origin.x + grid.cell_width * f32::from(col),
                                grid.origin.y + grid.line_height * f32::from(row),
                            );
                            let mut cursor = caret;
                            cursor.a = if focused { 0.8 } else { 0.35 };
                            window.paint_quad(fill(
                                Bounds::new(at, size(grid.cell_width, grid.line_height)),
                                cursor,
                            ));
                        }
                    }
                });
            },
        )
        .size_full();
        let mut body = v_flex()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .on_action(
                cx.listener(|this, _: &SendTab, _, cx| this.send_key(Key::Tab, Mods::NONE, cx)),
            )
            .on_action(cx.listener(|this, _: &SendBackTab, _, cx| {
                this.send_key(Key::BackTab, Mods::NONE, cx)
            }))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.on_key(event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    this.on_mouse_down(e, window, cx)
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, cx| this.on_mouse_up(e, cx)),
            )
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.on_scroll(e, cx)))
            .child(div().flex_1().child(grid));
        if let Some(code) = exited {
            let text = match code {
                Some(code) => format!("Shell exited (status {code}). Press Enter to reopen."),
                None => "Shell ended. Press Enter to reopen.".to_owned(),
            };
            body = body.child(div().px_2().text_sm().text_color(theme.warning).child(text));
        }
        body.into_any_element()
    }
}
