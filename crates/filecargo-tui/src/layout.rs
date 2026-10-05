//! Where everything goes. Pure arithmetic on a terminal size.

use ratatui::layout::{Constraint, Layout, Rect};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub tree: Option<Rect>,
    pub local: Rect,
    pub remote: Rect,
    pub bottom: Rect,
    pub status: Rect,
}

pub fn too_small(width: u16, height: u16) -> bool {
    width < MIN_WIDTH || height < MIN_HEIGHT
}

/// Panes on top, the bottom panel under them (30 % of the height, at least 6 rows), a one-row
/// status line last. With `maximize_bottom` the panel takes the whole body.
pub fn areas(size: Rect, tree_visible: bool, maximize_bottom: bool) -> Areas {
    let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(size);
    let nothing = Rect {
        width: 0,
        height: 0,
        ..body
    };
    if maximize_bottom {
        return Areas {
            tree: None,
            local: nothing,
            remote: nothing,
            bottom: body,
            status,
        };
    }
    let bottom_height = (body.height * 30 / 100)
        .max(6)
        .min(body.height.saturating_sub(4));
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).areas(body);
    let (tree, panes) = if tree_visible {
        let width = (top.width * 20 / 100).max(18);
        let [tree, panes] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(1)]).areas(top);
        (Some(tree), panes)
    } else {
        (None, top)
    };
    let [local, remote] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(panes);
    Areas {
        tree,
        local,
        remote,
        bottom,
        status,
    }
}

/// Rows a file pane can show: its height minus the border and the header.
pub fn pane_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn small_terminals_are_refused() {
        assert!(too_small(79, 24) && too_small(80, 23) && too_small(40, 10));
        assert!(!too_small(80, 24) && !too_small(200, 60));
    }

    #[test]
    fn the_tree_takes_a_fifth_of_a_wide_terminal_and_at_least_18_columns() {
        let wide = areas(rect(120, 40), true, false);
        assert_eq!(wide.tree.unwrap().width, 24);
        let narrower = areas(rect(85, 40), true, false);
        assert_eq!(narrower.tree.unwrap().width, 18);
        assert!(areas(rect(120, 40), false, false).tree.is_none());
    }

    #[test]
    fn the_bottom_panel_is_thirty_percent_with_a_floor_of_six_rows() {
        let big = areas(rect(120, 50), false, false); // body 49 rows
        assert_eq!(big.bottom.height, 14);
        let small = areas(rect(80, 24), false, false); // body 23 rows
        assert_eq!(small.bottom.height, 6);
        assert_eq!(small.status.height, 1);
        assert_eq!(big.local.height + big.bottom.height + 1, 50);
    }

    #[test]
    fn maximizing_gives_the_panel_the_whole_body() {
        let a = areas(rect(100, 30), true, true);
        assert_eq!(a.bottom.height, 29);
        assert_eq!((a.local.width, a.remote.width), (0, 0));
        assert!(a.tree.is_none());
    }

    #[test]
    fn panes_split_the_remaining_width_evenly() {
        let a = areas(rect(100, 30), true, false);
        assert!(a.local.width.abs_diff(a.remote.width) <= 1);
        assert_eq!(a.tree.unwrap().width + a.local.width + a.remote.width, 100);
    }
}
