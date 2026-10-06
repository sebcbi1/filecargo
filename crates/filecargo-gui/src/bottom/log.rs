//! The log tab: a virtualized list of the app's log lines that follows the newest one until
//! the user scrolls away.

use filecargo_app_core::prelude::*;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollStrategy,
    ScrollWheelEvent, Styled as _, UniformListScrollHandle, Window, div, px, uniform_list,
};

use crate::format::format_time;

pub struct LogView {
    log: LogBuffer,
    /// The site whose lines show besides the app-wide ones; `None` when not connected.
    pub scope: Option<SiteId>,
    pub scroll: UniformListScrollHandle,
    /// Lines the view has shown (to scroll when more arrive).
    shown: usize,
    pub follow: bool,
}

impl LogView {
    pub fn new(log: LogBuffer) -> Self {
        Self {
            log,
            scope: None,
            scroll: UniformListScrollHandle::default(),
            shown: 0,
            follow: true,
        }
    }

    /// Lines on show: the scope's and the app-wide ones.
    pub fn len(&self) -> usize {
        self.log.lines_for(self.scope).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `level` in the colour it deserves.
fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
        LogLevel::Trace => "TRACE",
    }
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = std::sync::Arc::new(self.log.lines_for(self.scope));
        let count = lines.len();
        if self.follow && count > 0 {
            self.scroll
                .scroll_to_item(count - 1, ScrollStrategy::Bottom);
        }
        self.shown = count;
        let theme = cx.theme().clone();
        div()
            .size_full()
            .font_family(theme.mono_font_family.clone())
            .text_size(px(12.))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                // scrolling up leaves follow mode; coming back to the bottom is `End` / the button
                let up = event.delta.pixel_delta(px(16.)).y > px(0.);
                if up {
                    this.follow = false;
                    cx.notify();
                }
            }))
            .child(
                uniform_list("log-lines", count, move |range, _window, _cx| {
                    {
                        lines
                            .iter()
                            .skip(range.start)
                            .take(range.len())
                            .enumerate()
                            .map(|(offset, line)| {
                                let color = match line.level {
                                    LogLevel::Error => theme.danger,
                                    LogLevel::Warn => theme.warning,
                                    LogLevel::Info => theme.foreground,
                                    LogLevel::Debug => theme.info,
                                    LogLevel::Trace => theme.muted_foreground,
                                };
                                h_flex()
                                    .id(("log", range.start + offset))
                                    .gap_2()
                                    .px_2()
                                    .child(
                                        div()
                                            .text_color(theme.muted_foreground)
                                            .child(format_time(Some(line.time))),
                                    )
                                    .child(div().text_color(color).child(level_label(line.level)))
                                    .child(
                                        div()
                                            .text_color(theme.muted_foreground)
                                            .child(line.target.clone()),
                                    )
                                    .child(div().flex_1().child(line.message.clone()))
                            })
                            .collect()
                    }
                })
                .track_scroll(&self.scroll)
                .size_full(),
            )
    }
}
