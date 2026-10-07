//! The window icon for X11 (`_NET_WM_ICON`). The other platforms take it from the macOS bundle,
//! the exe resources (see `build.rs`) or the Linux desktop file.

use std::sync::Arc;

const PNG: &[u8] = include_bytes!("../../../assets/icons/png/filecargo-256.png");

/// The 256 px logo, or `None` (after saying why) when it cannot be decoded: the window then
/// opens without an icon.
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    match image::load_from_memory_with_format(PNG, image::ImageFormat::Png) {
        Ok(decoded) => Some(Arc::new(decoded.into_rgba8())),
        Err(error) => {
            eprintln!("filecargo: cannot decode the window icon: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_the_256px_logo() {
        let icon = window_icon().expect("the embedded icon decodes");
        assert_eq!((icon.width(), icon.height()), (256, 256));
    }
}
