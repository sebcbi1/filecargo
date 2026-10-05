//! The terminal front-end: a pure reducer, a pure view and a thin loop.

pub mod app_loop;
pub mod bottom_view;
pub mod dialog;
pub mod dialog_util;
pub mod dialog_view;
pub mod form;
pub mod guard;
pub mod help;
pub mod help_view;
pub mod keymap;
pub mod keys;
pub mod layout;
mod pane;
pub mod prompt_ui;
pub mod prompt_view;
pub mod reducer;
#[cfg(test)]
mod test_support;
pub mod tree;
pub mod ui_state;
pub mod view;
#[cfg(test)]
mod view_tests;
