//! The terminal front-end: a pure reducer, a pure view and a thin loop.

pub mod app_loop;
pub mod guard;
pub mod keymap;
pub mod layout;
mod pane;
pub mod reducer;
#[cfg(test)]
mod test_support;
pub mod tree;
pub mod ui_state;
pub mod view;
#[cfg(test)]
mod view_tests;
