use std::time::Duration;

use anyhow::{Context as _, Result};
use filecargo_app_core::prelude::*;
use filecargo_gui::model::AppModel;
use filecargo_gui::workspace::{Quit, Workspace, bind_keys};
use gpui_kit::component::Theme;
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() -> Result<()> {
    let handle =
        App::start(StartOptions::default()).context("cannot start the application core")?;
    let for_ui = handle.clone();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            let app = for_ui;
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);
            bind_keys(cx);
            let quitting = app.clone();
            cx.on_action(move |_: &Quit, _cx| {
                // the app may ask for a confirmation first: the window closes when it has quit
                quitting.send(Command::Quit);
            });
            let model = AppModel::new(app, cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(780.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("filecargo".into()),
                    ..Default::default()
                }),
                app_id: Some("filecargo".into()),
                #[cfg(target_os = "linux")]
                icon: filecargo_gui::icon::window_icon(),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workspace::new(model, window, cx))
            }) {
                eprintln!("filecargo: cannot open the window: {error}");
                cx.quit();
            }
        });
    handle.shutdown(Duration::from_secs(3));
    Ok(())
}
