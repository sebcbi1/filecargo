//! The About dialog.

use std::sync::{Arc, LazyLock};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _, v_flex};
use gpui_kit::{App, Image, ImageFormat, ParentElement as _, Styled as _, Window, div, img, px};

pub const NAME: &str = "FileCargo";
pub const REPOSITORY: &str = "https://github.com/sebcbi1/filecargo";

/// Decoded once: the dialog's builder runs on every frame.
static LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        crate::icon::LOGO_PNG.to_vec(),
    ))
});

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
    window.open_dialog(cx, |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        dialog
            .w(px(360.))
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .pt_2()
                    .child(img(LOGO.clone()).size(px(96.)))
                    .child(div().pt_1().text_xl().font_semibold().child(NAME))
                    .children(
                        lines()
                            .into_iter()
                            .map(|line| div().text_sm().text_color(muted).child(line)),
                    ),
            )
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
