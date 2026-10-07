//! The embedded logo: shown in the About dialog, and the window icon for X11 (`_NET_WM_ICON`).
//! The other platforms take the window icon from the macOS bundle, the exe resources (see
//! `build.rs`) or the Linux desktop file.

/// The 256 px logo (PNG).
pub const LOGO_PNG: &[u8] = include_bytes!("../../../assets/icons/png/filecargo-256.png");

/// The 256 px logo, or `None` (after saying why) when it cannot be decoded: the window then
/// opens without an icon.
#[cfg(target_os = "linux")]
pub fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    match image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png) {
        Ok(decoded) => Some(std::sync::Arc::new(decoded.into_rgba8())),
        Err(error) => {
            eprintln!("filecargo: cannot decode the window icon: {error}");
            None
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_the_256px_logo() {
        let icon = window_icon().expect("the embedded icon decodes");
        assert_eq!((icon.width(), icon.height()), (256, 256));
    }
}
