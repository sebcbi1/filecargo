//! A tiny form toolkit: text, masked text, checkbox and select fields with one focus order.
//! ratatui has no form widgets; the site editor and every small dialog share this.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// Typed like text, shown as bullets.
    Masked,
    Checkbox,
    /// One of a fixed list; `Left` / `Right` / `Space` cycle.
    Select(Vec<&'static str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub text: String,
    /// Cursor position in `text`, counted in characters.
    pub cursor: usize,
    pub checked: bool,
    pub choice: usize,
    /// Hidden fields are skipped by the focus order and the renderer.
    pub visible: bool,
}

impl Field {
    fn base(id: &str, label: &str, kind: FieldKind) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
            kind,
            text: String::new(),
            cursor: 0,
            checked: false,
            choice: 0,
            visible: true,
        }
    }

    pub fn text(id: &str, label: &str, value: &str) -> Self {
        let mut field = Self::base(id, label, FieldKind::Text);
        field.set_text(value);
        field
    }

    pub fn masked(id: &str, label: &str) -> Self {
        Self::base(id, label, FieldKind::Masked)
    }

    pub fn checkbox(id: &str, label: &str, checked: bool) -> Self {
        let mut field = Self::base(id, label, FieldKind::Checkbox);
        field.checked = checked;
        field
    }

    pub fn select(
        id: &'static str,
        label: &'static str,
        choices: Vec<&'static str>,
        choice: usize,
    ) -> Self {
        let mut field = Self::base(id, label, FieldKind::Select(choices));
        field.choice = choice;
        field
    }

    pub fn set_text(&mut self, value: &str) {
        self.text = value.to_owned();
        self.cursor = value.chars().count();
    }

    fn is_text(&self) -> bool {
        matches!(self.kind, FieldKind::Text | FieldKind::Masked)
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_index(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let at = self.byte_index(self.cursor - 1);
            self.text.remove(at);
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let at = self.byte_index(self.cursor);
            self.text.remove(at);
        }
    }

    /// The selected choice of a `Select`.
    pub fn chosen(&self) -> &'static str {
        match &self.kind {
            FieldKind::Select(choices) => choices.get(self.choice).copied().unwrap_or(""),
            _ => "",
        }
    }
}

/// What a key did to a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormOutcome {
    Handled,
    Submit,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub fields: Vec<Field>,
    /// Index into `fields` of the focused (visible) field.
    pub focus: usize,
    /// Shown under the fields; cleared by the next key.
    pub error: Option<String>,
}

impl Form {
    pub fn new(fields: Vec<Field>) -> Self {
        let mut form = Self {
            fields,
            focus: 0,
            error: None,
        };
        form.fix_focus();
        form
    }

    pub fn field(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn field_mut(&mut self, id: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.id == id)
    }

    pub fn text_of(&self, id: &str) -> &str {
        self.field(id).map_or("", |f| f.text.trim())
    }

    pub fn checked(&self, id: &str) -> bool {
        self.field(id).is_some_and(|f| f.checked)
    }

    pub fn choice(&self, id: &str) -> usize {
        self.field(id).map_or(0, |f| f.choice)
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) {
        if let Some(field) = self.field_mut(id) {
            field.visible = visible;
        }
        self.fix_focus();
    }

    /// Moves the focus onto a visible field if it sits on a hidden one.
    fn fix_focus(&mut self) {
        if self.fields.get(self.focus).is_some_and(|f| f.visible) {
            return;
        }
        self.focus = self.fields.iter().position(|f| f.visible).unwrap_or(0);
    }

    fn step_focus(&mut self, forward: bool) {
        let visible: Vec<usize> = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.visible)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }
        let at = visible.iter().position(|i| *i == self.focus).unwrap_or(0);
        let next = if forward {
            (at + 1) % visible.len()
        } else {
            (at + visible.len() - 1) % visible.len()
        };
        self.focus = visible[next];
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(field) = self.fields.get_mut(self.focus).filter(|f| f.is_text()) {
            for c in text.chars().filter(|c| !c.is_control()) {
                field.insert(c);
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> FormOutcome {
        self.error = None;
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        match key.code {
            KeyCode::Esc => return FormOutcome::Cancel,
            KeyCode::Enter => return FormOutcome::Submit,
            KeyCode::Tab | KeyCode::Down => {
                self.step_focus(true);
                return FormOutcome::Handled;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.step_focus(false);
                return FormOutcome::Handled;
            }
            _ => {}
        }
        let Some(field) = self.fields.get_mut(self.focus) else {
            return FormOutcome::Handled;
        };
        match &field.kind {
            FieldKind::Text | FieldKind::Masked => match key.code {
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    field.set_text("")
                }
                KeyCode::Char(c) if plain => field.insert(c),
                KeyCode::Backspace => field.backspace(),
                KeyCode::Delete => field.delete(),
                KeyCode::Left => field.cursor = field.cursor.saturating_sub(1),
                KeyCode::Right => field.cursor = (field.cursor + 1).min(field.text.chars().count()),
                KeyCode::Home => field.cursor = 0,
                KeyCode::End => field.cursor = field.text.chars().count(),
                _ => {}
            },
            FieldKind::Checkbox => {
                if matches!(key.code, KeyCode::Char(' ')) {
                    field.checked = !field.checked;
                }
            }
            FieldKind::Select(choices) => {
                let count = choices.len().max(1);
                match key.code {
                    KeyCode::Right | KeyCode::Char(' ') => {
                        field.choice = (field.choice + 1) % count
                    }
                    KeyCode::Left => field.choice = (field.choice + count - 1) % count,
                    _ => {}
                }
            }
        }
        FormOutcome::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(form: &mut Form, text: &str) {
        for c in text.chars() {
            form.on_key(key(KeyCode::Char(c)));
        }
    }

    fn sample() -> Form {
        Form::new(vec![
            Field::text("name", "Name", ""),
            Field::masked("password", "Password"),
            Field::checkbox("remember", "Remember", false),
            Field::select("protocol", "Protocol", vec!["SFTP", "FTP", "FTPS"], 0),
        ])
    }

    #[test]
    fn typing_editing_and_cursor_movement_work_on_unicode_text() {
        let mut form = sample();
        type_text(&mut form, "héllo");
        assert_eq!(form.text_of("name"), "héllo");
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Backspace));
        assert_eq!(
            form.text_of("name"),
            "hélo",
            "backspace removed the l before the cursor"
        );
        form.on_key(key(KeyCode::Delete));
        assert_eq!(form.text_of("name"), "héo");
        form.on_key(key(KeyCode::Home));
        type_text(&mut form, "✓");
        assert_eq!(form.text_of("name"), "✓héo");
        form.on_key(key(KeyCode::End));
        type_text(&mut form, "!");
        assert_eq!(form.text_of("name"), "✓héo!");
        form.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(form.text_of("name"), "");
    }

    #[test]
    fn tab_and_arrows_cycle_through_visible_fields_only() {
        let mut form = sample();
        assert_eq!(form.focus, 0);
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.focus, 1);
        form.set_visible("remember", false);
        form.on_key(key(KeyCode::Down));
        assert_eq!(form.focus, 3, "the hidden checkbox is skipped");
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.focus, 0, "wraps around");
        form.on_key(key(KeyCode::BackTab));
        assert_eq!(form.focus, 3);
        form.on_key(key(KeyCode::Up));
        assert_eq!(form.focus, 1);
    }

    #[test]
    fn hiding_the_focused_field_moves_the_focus() {
        let mut form = sample();
        form.on_key(key(KeyCode::Tab));
        form.set_visible("password", false);
        assert_eq!(form.focus, 0);
    }

    #[test]
    fn checkboxes_toggle_and_selects_cycle_both_ways() {
        let mut form = sample();
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Char(' ')));
        assert!(form.checked("remember"));
        form.on_key(key(KeyCode::Char(' ')));
        assert!(!form.checked("remember"));
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Right));
        assert_eq!(form.field("protocol").unwrap().chosen(), "FTP");
        form.on_key(key(KeyCode::Left));
        form.on_key(key(KeyCode::Left));
        assert_eq!(
            form.field("protocol").unwrap().chosen(),
            "FTPS",
            "wraps backwards"
        );
    }

    #[test]
    fn enter_submits_and_escape_cancels_from_any_field() {
        let mut form = sample();
        assert_eq!(form.on_key(key(KeyCode::Enter)), FormOutcome::Submit);
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.on_key(key(KeyCode::Esc)), FormOutcome::Cancel);
        assert_eq!(form.on_key(key(KeyCode::Char('x'))), FormOutcome::Handled);
    }

    #[test]
    fn paste_inserts_printable_text_only_into_text_fields() {
        let mut form = sample();
        form.paste("pa\nste\t d");
        assert_eq!(form.text_of("name"), "paste d");
        form.on_key(key(KeyCode::Tab));
        form.paste("secret");
        assert_eq!(form.field("password").unwrap().text, "secret");
        form.on_key(key(KeyCode::Tab));
        form.paste("ignored by a checkbox");
        assert!(!form.checked("remember"));
    }

    #[test]
    fn an_error_is_cleared_by_the_next_key() {
        let mut form = sample();
        form.error = Some("bad".to_owned());
        form.on_key(key(KeyCode::Char('a')));
        assert!(form.error.is_none());
    }
}
