#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Headless harness: an app-core with a temp config dir and an in-memory keychain, and a gpui
//! test window holding the real `Workspace`.

use std::sync::Arc;
use std::time::Duration;

use filecargo_app_core::prelude::*;
use filecargo_config::MemoryStore;
use filecargo_gui::model::AppModel;
use filecargo_gui::workspace::Workspace;
use gpui_kit::AppContext as _;
use gpui_kit::base::Root;
use gpui_kit::{
    Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions, px, size,
};

pub struct Harness {
    pub app: AppHandle,
    pub window: WindowHandle<Root>,
    pub workspace: Entity<Workspace>,
    pub model: Entity<AppModel>,
    pub _config: tempfile::TempDir,
}

pub fn start_app(options: impl FnOnce(&mut StartOptions)) -> (AppHandle, tempfile::TempDir) {
    let config = tempfile::tempdir().unwrap();
    let mut start = StartOptions {
        paths: Some(Paths::from_override(Some(config.path().to_path_buf()))),
        secrets: Some(Arc::new(MemoryStore::new())),
        connector: None,
    };
    options(&mut start);
    (App::start(start).unwrap(), config)
}

pub fn open(cx: &mut TestAppContext) -> Harness {
    open_with(cx, |_| {})
}

pub fn open_with(cx: &mut TestAppContext, options: impl FnOnce(&mut StartOptions)) -> Harness {
    let (app, config) = start_app(options);
    cx.update(gpui_kit::init);
    let handle = app.clone();
    let (window, workspace, model) = cx.update(|cx| {
        let model = AppModel::new(handle, cx);
        let model_for_view = model.clone();
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1280.), px(780.)),
        };
        let (window, workspace) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(model_for_view, window, cx)),
        )
        .unwrap();
        (window.downcast::<Root>().unwrap(), workspace, model)
    });
    Harness {
        app,
        window,
        workspace,
        model,
        _config: config,
    }
}

impl Harness {
    /// Waits (real time: the app core runs on its own threads) until `condition` holds for the
    /// latest snapshot the model has taken.
    pub async fn wait_state(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        condition: impl Fn(&AppState) -> bool,
    ) {
        for _ in 0..400 {
            cx.executor().advance_clock(filecargo_gui::model::POLL * 2);
            cx.run_until_parked();
            if cx.read_entity(&self.model, |m, _| condition(&m.state)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out waiting for {what}");
    }
}
