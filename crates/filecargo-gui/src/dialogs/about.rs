//! The About dialog.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::{App, ParentElement as _, Styled as _, Window, px};

pub const NAME: &str = "FileCargo";
pub const REPOSITORY: &str = "https://github.com/sebcbi1/filecargo";

/// The lines the dialog shows under its title.
pub fn lines() -> Vec<String> {
    vec![
        format!("Version {}", env!("CARGO_PKG_VERSION")),
        "FTP, FTPS and SFTP client.".to_owned(),
        "Licensed under MIT or Apache-2.0.".to_owned(),
        REPOSITORY.to_owned(),
    ]
}

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .title(NAME)
            .w(px(360.))
            .child(v_flex().gap_1().children(lines()))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("about-repository")
                            .label("Repository")
                            .on_click(|_, _, cx| cx.open_url(REPOSITORY)),
                    )
                    .child(DialogClose::new().trigger(|button| button.primary().label("Close"))),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_shows_the_crate_version_and_the_repository() {
        let lines = lines();
        assert_eq!(lines[0], format!("Version {}", env!("CARGO_PKG_VERSION")));
        assert!(lines.iter().any(|l| l == REPOSITORY));
    }
}
