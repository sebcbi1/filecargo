//! The settings dialog.

use filecargo_app_core::prelude::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use super::open_form;
use crate::model::AppModel;

pub const RULES: [(&str, ConflictRule); 6] = [
    ("Ask each time", ConflictRule::Ask),
    ("Overwrite", ConflictRule::Overwrite),
    ("Overwrite if newer", ConflictRule::OverwriteIfNewer),
    ("Resume", ConflictRule::Resume),
    ("Skip", ConflictRule::Skip),
    ("Keep both", ConflictRule::Rename),
];

pub const LEVELS: [(&str, LogLevel); 5] = [
    ("Error", LogLevel::Error),
    ("Warning", LogLevel::Warn),
    ("Info", LogLevel::Info),
    ("Debug", LogLevel::Debug),
    ("Trace", LogLevel::Trace),
];

/// What the controls hold.
#[derive(Debug, Clone)]
pub struct SettingsForm {
    pub max_concurrent: String,
    pub conflict: usize,
    pub timeout: String,
    pub keepalive: String,
    pub show_hidden: bool,
    pub confirm_delete: bool,
    pub local_start_dir: String,
    pub log_level: usize,
}

impl SettingsForm {
    pub fn of(settings: &Settings) -> Self {
        Self {
            max_concurrent: settings.transfers.max_concurrent.to_string(),
            conflict: RULES
                .iter()
                .position(|r| r.1 == settings.transfers.default_conflict)
                .unwrap_or(0),
            timeout: settings.connection.timeout_secs.to_string(),
            keepalive: settings.connection.keepalive_secs.to_string(),
            show_hidden: settings.ui.show_hidden,
            confirm_delete: settings.ui.confirm_delete,
            local_start_dir: settings.ui.local_start_dir.clone(),
            log_level: LEVELS
                .iter()
                .position(|l| l.1 == settings.log.level)
                .unwrap_or(2),
        }
    }

    /// `base` with the form's values, or what is wrong.
    pub fn to_settings(&self, base: &Settings) -> Result<Settings, String> {
        let mut settings = base.clone();
        settings.transfers.max_concurrent = match self.max_concurrent.trim().parse::<u8>() {
            Ok(n @ 1..=10) => n,
            _ => return Err("Simultaneous transfers: a number from 1 to 10.".to_owned()),
        };
        settings.transfers.default_conflict =
            RULES.get(self.conflict).map_or(ConflictRule::Ask, |r| r.1);
        let seconds = |text: &str, what: &str| match text.trim().parse::<u32>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!("{what}: a number of seconds above 0.")),
        };
        settings.connection.timeout_secs = seconds(&self.timeout, "Connection timeout")?;
        settings.connection.keepalive_secs = seconds(&self.keepalive, "Keep-alive")?;
        settings.ui.show_hidden = self.show_hidden;
        settings.ui.confirm_delete = self.confirm_delete;
        let start = self.local_start_dir.trim();
        settings.ui.local_start_dir = if start.is_empty() {
            "~".to_owned()
        } else {
            start.to_owned()
        };
        settings.log.level = LEVELS.get(self.log_level).map_or(LogLevel::Info, |l| l.1);
        Ok(settings)
    }
}

pub struct SettingsView {
    model: Entity<AppModel>,
    pub max_concurrent: Entity<InputState>,
    pub timeout: Entity<InputState>,
    pub keepalive: Entity<InputState>,
    pub local_start_dir: Entity<InputState>,
    pub conflict: Entity<SelectState<SearchableVec<&'static str>>>,
    pub log_level: Entity<SelectState<SearchableVec<&'static str>>>,
    show_hidden: bool,
    confirm_delete: bool,
    pub error: Option<String>,
}

fn text(window: &mut Window, cx: &mut Context<SettingsView>, value: &str) -> Entity<InputState> {
    let value = value.to_owned();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx);
        state.set_value(value, window, cx);
        state
    })
}

fn choose(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    names: Vec<&'static str>,
    at: usize,
) -> Entity<SelectState<SearchableVec<&'static str>>> {
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(names),
            Some(IndexPath::new(at)),
            window,
            cx,
        )
    })
}

impl SettingsView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = SettingsForm::of(&model.read(cx).state.settings);
        let max_concurrent = text(window, cx, &form.max_concurrent);
        super::focus_later(window, cx, &max_concurrent);
        Self {
            max_concurrent,
            timeout: text(window, cx, &form.timeout),
            keepalive: text(window, cx, &form.keepalive),
            local_start_dir: text(window, cx, &form.local_start_dir),
            conflict: choose(
                window,
                cx,
                RULES.iter().map(|r| r.0).collect(),
                form.conflict,
            ),
            log_level: choose(
                window,
                cx,
                LEVELS.iter().map(|l| l.0).collect(),
                form.log_level,
            ),
            show_hidden: form.show_hidden,
            confirm_delete: form.confirm_delete,
            error: None,
            model,
        }
    }

    pub fn set_show_hidden(&mut self, value: bool, cx: &mut Context<Self>) {
        self.show_hidden = value;
        cx.notify();
    }

    fn form(&self, cx: &App) -> SettingsForm {
        let read = |state: &Entity<InputState>| state.read(cx).value().to_string();
        let index = |select: &Entity<SelectState<SearchableVec<&'static str>>>,
                     names: Vec<&'static str>| {
            select
                .read(cx)
                .selected_value()
                .and_then(|v| names.iter().position(|n| n == v))
                .unwrap_or(0)
        };
        SettingsForm {
            max_concurrent: read(&self.max_concurrent),
            conflict: index(&self.conflict, RULES.iter().map(|r| r.0).collect()),
            timeout: read(&self.timeout),
            keepalive: read(&self.keepalive),
            show_hidden: self.show_hidden,
            confirm_delete: self.confirm_delete,
            local_start_dir: read(&self.local_start_dir),
            log_level: index(&self.log_level, LEVELS.iter().map(|l| l.0).collect()),
        }
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        let base = self.model.read(cx).state.settings.clone();
        match self.form(cx).to_settings(&base) {
            Ok(settings) => {
                self.model.read(cx).send(Command::UpdateSettings(settings));
                true
            }
            Err(message) => {
                self.error = Some(message);
                cx.notify();
                false
            }
        }
    }

    fn row(label: &'static str, control: impl IntoElement) -> gpui_kit::Div {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(200.)).text_sm().child(label))
            .child(div().flex_1().child(control))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .gap_2()
            .child(Self::row(
                "Simultaneous transfers",
                Input::new(&self.max_concurrent).id("set-concurrent"),
            ))
            .child(Self::row(
                "When a file exists",
                Select::new(&self.conflict).id("set-conflict"),
            ))
            .child(Self::row(
                "Connection timeout (s)",
                Input::new(&self.timeout).id("set-timeout"),
            ))
            .child(Self::row(
                "Keep-alive (s)",
                Input::new(&self.keepalive).id("set-keepalive"),
            ))
            .child(Self::row(
                "Local start folder",
                Input::new(&self.local_start_dir).id("set-start"),
            ))
            .child(Self::row(
                "Log level",
                Select::new(&self.log_level).id("set-log"),
            ))
            .child(
                Checkbox::new("set-hidden")
                    .label("Show hidden files")
                    .checked(self.show_hidden)
                    .on_click(cx.listener(|this, v: &bool, _, cx| this.set_show_hidden(*v, cx))),
            )
            .child(
                Checkbox::new("set-confirm")
                    .label("Ask before deleting")
                    .checked(self.confirm_delete)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        this.confirm_delete = *v;
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

pub fn open(model: Entity<AppModel>, window: &mut Window, cx: &mut App) -> Entity<SettingsView> {
    let view = cx.new(|cx| SettingsView::new(model, window, cx));
    open_form(
        window,
        cx,
        "Settings",
        px(520.),
        view.clone(),
        "Save",
        |view, cx| view.update(cx, |view, cx| view.submit(cx)),
    );
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_round_trips_the_defaults() {
        let base = Settings::default();
        assert_eq!(SettingsForm::of(&base).to_settings(&base).unwrap(), base);
    }

    #[test]
    fn bad_numbers_are_refused_with_their_name() {
        let base = Settings::default();
        for (field, bad) in [
            ("Simultaneous", "0"),
            ("Simultaneous", "11"),
            ("timeout", "0"),
            ("Keep-alive", "x"),
        ] {
            let mut form = SettingsForm::of(&base);
            match field {
                "Simultaneous" => form.max_concurrent = bad.into(),
                "timeout" => form.timeout = bad.into(),
                _ => form.keepalive = bad.into(),
            }
            let error = form.to_settings(&base).unwrap_err();
            assert!(
                error.contains(field) || error.contains("timeout"),
                "{error}"
            );
        }
    }

    #[test]
    fn choices_and_flags_land_in_the_settings() {
        let base = Settings::default();
        let mut form = SettingsForm::of(&base);
        form.conflict = 4;
        form.log_level = 3;
        form.show_hidden = true;
        form.confirm_delete = false;
        form.local_start_dir = "  ".into();
        let settings = form.to_settings(&base).unwrap();
        assert_eq!(settings.transfers.default_conflict, ConflictRule::Skip);
        assert_eq!(settings.log.level, LogLevel::Debug);
        assert!(settings.ui.show_hidden && !settings.ui.confirm_delete);
        assert_eq!(
            settings.ui.local_start_dir, "~",
            "an empty folder means home"
        );
    }
}
