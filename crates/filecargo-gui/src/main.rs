use std::time::Duration;

use anyhow::{Context as _, Result};
use filecargo_app_core::prelude::*;
use filecargo_gui::model::AppModel;
use filecargo_gui::workspace::{About, Quit, Workspace, bind_keys};
use gpui_kit::component::Theme;
use gpui_kit::{
    App as GpuiApp, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

fn main() -> Result<()> {
    let handle =
        App::start(StartOptions::default()).context("cannot start the application core")?;
    let for_ui = handle.clone();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    // the Dock icon of an app without a window brings the window back (macOS)
    application.on_reopen(|cx| {
        if cx.windows().is_empty()
            && let Some(MainModel(model)) = cx.try_global::<MainModel>().cloned()
        {
            let _ = open_main_window(&model, cx);
        }
    });
    application.run(move |cx| {
        let app = for_ui;
        gpui_kit::init(cx);
        Theme::sync_system_appearance(None, cx);
        bind_keys(cx);
        let model = AppModel::new(app.clone(), cx);
        // the global keeps the model (and its snapshot task) alive while there is no window
        cx.set_global(MainModel(model.clone()));
        // Quit: with the window closed (macOS keeps the app alive) it reopens first, so the
        // "transfers still running" question has a window to be asked in
        let (quitting, quit_model) = (app, model.clone());
        cx.on_action(move |_: &Quit, cx| {
            if cx.windows().is_empty() && open_main_window(&quit_model, cx).is_err() {
                cx.quit();
            }
            // the app may ask for a confirmation first: the window closes when it has quit
            quitting.send(Command::Quit);
        });
        // app-wide, so the macOS menu item is enabled whatever has the focus (or with no window)
        let about_model = model.clone();
        cx.on_action(move |_: &About, cx| {
            let model = about_model.clone();
            // the menu dispatches while the active window is being updated: open it afterwards
            cx.defer(move |cx| {
                let window = match cx.active_window().or_else(|| cx.windows().first().copied()) {
                    Some(window) => window,
                    None => match open_main_window(&model, cx) {
                        Ok(window) => window,
                        Err(_) => return,
                    },
                };
                window
                    .update(cx, |_, window, cx| {
                        filecargo_gui::dialogs::about::open(window, cx)
                    })
                    .ok();
            });
        });
        #[cfg(target_os = "macos")]
        {
            use gpui_kit::{Menu, MenuItem};
            cx.set_menus([Menu {
                name: "FileCargo".into(),
                items: vec![
                    MenuItem::action("About FileCargo", About),
                    MenuItem::separator(),
                    MenuItem::action("Quit FileCargo", Quit),
                ],
                disabled: false,
            }]);
        }
        if let Err(error) = open_main_window(&model, cx) {
            eprintln!("filecargo: cannot open the window: {error}");
            cx.quit();
        }
    });
    handle.shutdown(Duration::from_secs(3));
    Ok(())
}

/// The model every window of the application shows.
#[derive(Clone)]
struct MainModel(gpui_kit::Entity<AppModel>);

impl gpui_kit::Global for MainModel {}

/// Opens the main window on `model`.
fn open_main_window(
    model: &gpui_kit::Entity<AppModel>,
    cx: &mut GpuiApp,
) -> Result<gpui_kit::AnyWindowHandle> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1280.), px(780.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("FileCargo".into()),
            ..Default::default()
        }),
        app_id: Some("filecargo".into()),
        #[cfg(target_os = "linux")]
        icon: filecargo_gui::icon::window_icon(),
        ..Default::default()
    };
    let model = model.clone();
    let (window, _) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| Workspace::new(model, window, cx))
    })?;
    Ok(window)
}
