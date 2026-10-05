use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use filecargo_app_core::prelude::*;
use filecargo_tui::{app_loop, guard, view};

const USAGE: &str = "usage: filecargo-tui [--config-dir <path>] [--version]";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("filecargo-tui {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--config-dir" => {
                config_dir = Some(args.next().context("--config-dir needs a path")?.into());
            }
            other => anyhow::bail!("unknown argument {other:?}\n{USAGE}"),
        }
    }

    let app = App::start(StartOptions {
        paths: config_dir.map(|dir| Paths::from_override(Some(dir))),
        ..StartOptions::default()
    })?;
    init_logging(&app.log());

    let mut terminal = guard::enter().context("cannot set up the terminal")?;
    let result = app.runtime().block_on(app_loop::run(
        &app,
        &mut terminal,
        &mut |frame, ui, state| {
            view::render(frame, ui, state);
        },
    ));
    guard::leave();
    app.shutdown(Duration::from_secs(3));
    result.context("terminal error")
}
