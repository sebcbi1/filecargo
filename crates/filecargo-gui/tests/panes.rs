#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;

use filecargo_app_core::prelude::*;
use gpui_kit::component::table::ColumnSort;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use support::Harness;

/// `dir/` with `sub/`, `a.txt`, `b.txt`, `c.txt`; rows of the local pane: 0 `..`, 1 sub, 2 a, 3 b, 4 c.
fn local_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), name).unwrap();
    }
    dir
}

async fn show_local(h: &Harness, cx: &mut TestAppContext, dir: &Path) {
    h.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: dir.display().to_string(),
    });
    // the listing reports the canonical path (`/private/var` on macOS, no `\\?\` on Windows)
    let wanted = std::fs::canonicalize(dir).unwrap();
    let wanted = match wanted.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) => std::path::PathBuf::from(rest),
        None => wanted,
    };

    h.wait_state(cx, "the local listing", move |s| {
        s.local.path == wanted && s.local.entries.len() >= 4
    })
    .await;
    cx.run_until_parked();
}

fn selected(cx: &mut TestAppContext, h: &Harness) -> Vec<String> {
    let table = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&table, |p, _| p.table.clone());
    cx.read_entity(&table, |t, _| {
        t.delegate().selected.iter().cloned().collect()
    })
}

fn click_row(cx: &mut TestAppContext, h: &Harness, row: usize, modifiers: Modifiers) {
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    table.update(cx, |t, cx| {
        t.delegate_mut().click_row(row, modifiers);
        cx.notify();
    });
}

const PLAIN: Modifiers = Modifiers {
    control: false,
    alt: false,
    shift: false,
    platform: false,
    function: false,
};

fn secondary() -> Modifiers {
    // `secondary` is Ctrl on Linux and Windows, Cmd on macOS
    if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..PLAIN
        }
    } else {
        Modifiers {
            control: true,
            ..PLAIN
        }
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..PLAIN
    }
}

#[gpui_kit::test]
async fn the_local_pane_lists_the_directory_with_a_parent_row(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let dir = local_tree();
    show_local(&h, cx, dir.path()).await;
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    let names: Vec<String> = cx.read_entity(&table, |t, _| {
        (0..6)
            .filter_map(|row| t.delegate().entry(row).map(|e| e.name.clone()))
            .collect()
    });
    assert_eq!(names, ["sub", "a.txt", "b.txt", "c.txt"]);
    assert!(cx.read_entity(&table, |t, _| t.delegate().has_parent));
}

#[gpui_kit::test]
async fn plain_secondary_and_shift_clicks_build_a_multi_selection(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let dir = local_tree();
    show_local(&h, cx, dir.path()).await;

    click_row(cx, &h, 2, PLAIN);
    assert_eq!(selected(cx, &h), ["a.txt"]);
    click_row(cx, &h, 4, secondary());
    assert_eq!(selected(cx, &h), ["a.txt", "c.txt"], "secondary adds");
    click_row(cx, &h, 2, secondary());
    assert_eq!(selected(cx, &h), ["c.txt"], "secondary toggles off");
    click_row(cx, &h, 2, PLAIN);
    click_row(cx, &h, 4, shift());
    assert_eq!(
        selected(cx, &h),
        ["a.txt", "b.txt", "c.txt"],
        "shift selects the range"
    );
    click_row(cx, &h, 3, PLAIN);
    assert_eq!(
        selected(cx, &h),
        ["b.txt"],
        "a plain click selects only that row"
    );
    click_row(cx, &h, 0, PLAIN);
    assert!(selected(cx, &h).is_empty(), "the `..` row selects nothing");
}

#[gpui_kit::test]
async fn double_clicking_a_directory_navigates_into_it_and_the_parent_row_goes_up(
    cx: &mut TestAppContext,
) {
    let h = support::open(cx);
    let dir = local_tree();
    show_local(&h, cx, dir.path()).await;
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.double_click(("row", 1usize), cx);
    })
    .unwrap();
    let inside = dir.path().join("sub");
    h.wait_state(cx, "to enter sub", {
        let inside = inside.clone();
        move |s| s.local.path == inside
    })
    .await;
    cx.run_until_parked();
    cx.update_window(h.window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.double_click(("row", 0usize), cx);
    })
    .unwrap();
    let parent = dir.path().to_path_buf();
    h.wait_state(cx, "to go up", move |s| s.local.path == parent)
        .await;
}

#[gpui_kit::test]
async fn the_selection_survives_a_refresh_of_names_that_exist_and_resets_on_navigation(
    cx: &mut TestAppContext,
) {
    let h = support::open(cx);
    let dir = local_tree();
    show_local(&h, cx, dir.path()).await;
    click_row(cx, &h, 2, PLAIN);
    click_row(cx, &h, 4, secondary());
    let generation = cx.read_entity(&h.model, |m, _| m.state.local.generation);

    std::fs::remove_file(dir.path().join("c.txt")).unwrap();
    h.app.send(Command::Refresh(PaneId::Local));
    h.wait_state(cx, "the refresh", move |s| s.local.generation > generation)
        .await;
    cx.run_until_parked();
    assert_eq!(
        selected(cx, &h),
        ["a.txt"],
        "c.txt is gone, a.txt stays selected"
    );

    h.app.send(Command::Navigate {
        pane: PaneId::Local,
        path: "sub".into(),
    });
    let inside = dir.path().join("sub");
    h.wait_state(cx, "to enter sub", move |s| s.local.path == inside)
        .await;
    cx.run_until_parked();
    assert!(
        selected(cx, &h).is_empty(),
        "navigation clears the selection"
    );
}

#[gpui_kit::test]
async fn a_header_click_sorts_through_the_app(cx: &mut TestAppContext) {
    let h = support::open(cx);
    let dir = local_tree();
    show_local(&h, cx, dir.path()).await;
    let pane = cx.read_entity(&h.workspace, |w, _| w.local.clone());
    let table = cx.read_entity(&pane, |p, _| p.table.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        table.update(cx, |t, cx| {
            use gpui_kit::component::table::TableDelegate as _;
            t.delegate_mut()
                .perform_sort(1, ColumnSort::Descending, window, cx);
        });
    })
    .unwrap();
    h.wait_state(cx, "the new sort", |s| {
        s.local.sort
            == Sort {
                key: SortKey::Size,
                ascending: false,
            }
    })
    .await;
}

#[gpui_kit::test]
async fn the_remote_pane_shows_a_hint_until_a_session_exists(cx: &mut TestAppContext) {
    let h = support::open(cx);
    cx.run_until_parked();
    // not connected: the pane has no listing to show
    assert!(cx.read_entity(&h.model, |m, _| m.state.remote.is_none()));
}
