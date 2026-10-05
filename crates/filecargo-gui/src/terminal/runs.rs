//! A terminal screen row as drawing instructions: text segments with style runs and background
//! rectangles, all in cell units. Pure (no gpui types), so the attribute boundaries, colours and
//! wide characters are unit-tested.

use filecargo_app_core::prelude::{Color, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(self) -> u32 {
        (u32::from(self.0) << 16) | (u32::from(self.1) << 8) | u32::from(self.2)
    }
}

/// The xterm palette: 16 named colours, a 6×6×6 cube, 24 greys.
pub fn palette(index: u8) -> Rgb {
    const NAMED: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    match index {
        0..=15 => {
            let hex = NAMED[usize::from(index)];
            Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
        }
        16..=231 => {
            let n = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        232..=255 => {
            let grey = 8 + 10 * (index - 232);
            Rgb(grey, grey, grey)
        }
    }
}

/// What a cell looks like once colours are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A piece of text starting at cell `col`, `cells` wide. A wide character is a segment of its
/// own so the monospace grid keeps its alignment; everything else on a row is one segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub col: usize,
    pub cells: usize,
    pub text: String,
    /// `(bytes, style)` runs covering `text`.
    pub runs: Vec<(usize, Style)>,
}

/// A rectangle of one background colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Background {
    pub col: usize,
    pub cells: usize,
    pub color: Rgb,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowLayout {
    pub segments: Vec<Segment>,
    pub backgrounds: Vec<Background>,
}

fn resolve(color: Color, default: Rgb) -> Rgb {
    match color {
        Color::Default => default,
        Color::Idx(index) => palette(index),
        Color::Rgb(r, g, b) => Rgb(r, g, b),
    }
}

fn dim(color: Rgb) -> Rgb {
    let scale = |v: u8| (f32::from(v) * 0.6) as u8;
    Rgb(scale(color.0), scale(color.1), scale(color.2))
}

/// Lays out row `row` of `screen` (`cols` cells wide).
pub fn row_layout(
    screen: &Screen,
    row: u16,
    cols: u16,
    default_fg: Rgb,
    default_bg: Rgb,
) -> RowLayout {
    let mut layout = RowLayout::default();
    let mut current: Option<Segment> = None;
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let mut fg = resolve(cell.fgcolor(), default_fg);
        let mut bg = match cell.bgcolor() {
            Color::Default => None,
            other => Some(resolve(other, default_bg)),
        };
        if cell.inverse() {
            let old_bg = bg.unwrap_or(default_bg);
            bg = Some(fg);
            fg = old_bg;
        }
        if cell.dim() {
            fg = dim(fg);
        }
        let cells = if cell.is_wide() { 2 } else { 1 };
        if let Some(color) = bg {
            match layout.backgrounds.last_mut() {
                Some(last) if last.color == color && last.col + last.cells == usize::from(col) => {
                    last.cells += cells;
                }
                _ => layout.backgrounds.push(Background {
                    col: usize::from(col),
                    cells,
                    color,
                }),
            }
        }
        let style = Style {
            fg,
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
        };
        let text = if cell.has_contents() {
            cell.contents()
        } else {
            " "
        };
        if cell.is_wide() {
            layout.segments.extend(current.take());
            layout.segments.push(Segment {
                col: usize::from(col),
                cells,
                text: text.to_owned(),
                runs: vec![(text.len(), style)],
            });
            continue;
        }
        match current.as_mut() {
            Some(segment) => {
                segment.text.push_str(text);
                segment.cells += 1;
                match segment.runs.last_mut() {
                    Some((len, last)) if *last == style => *len += text.len(),
                    _ => segment.runs.push((text.len(), style)),
                }
            }
            None => {
                current = Some(Segment {
                    col: usize::from(col),
                    cells: 1,
                    text: text.to_owned(),
                    runs: vec![(text.len(), style)],
                });
            }
        }
    }
    layout.segments.extend(current);
    layout
}

#[cfg(test)]
mod tests {
    use filecargo_app_core::prelude::Parser;

    use super::*;

    const FG: Rgb = Rgb(0xdd, 0xdd, 0xdd);
    const BG: Rgb = Rgb(0x11, 0x11, 0x11);

    fn layout(input: &str, cols: u16) -> RowLayout {
        let mut parser = Parser::new(3, cols, 0);
        parser.process(input.as_bytes());
        row_layout(parser.screen(), 0, cols, FG, BG)
    }

    #[test]
    fn the_palette_covers_named_cube_and_grey_colours() {
        assert_eq!(palette(1), Rgb(0xcd, 0, 0));
        assert_eq!(palette(15), Rgb(0xff, 0xff, 0xff));
        assert_eq!(palette(16), Rgb(0, 0, 0));
        assert_eq!(palette(21), Rgb(0, 0, 255));
        assert_eq!(palette(231), Rgb(255, 255, 255));
        assert_eq!(palette(232), Rgb(8, 8, 8));
        assert_eq!(palette(255), Rgb(238, 238, 238));
        assert_eq!(Rgb(1, 2, 3).hex(), 0x010203);
    }

    #[test]
    fn plain_text_is_one_segment_padded_to_the_row_width() {
        let layout = layout("hi", 6);
        assert_eq!(layout.segments.len(), 1);
        assert_eq!(layout.segments[0].text, "hi    ");
        assert_eq!((layout.segments[0].col, layout.segments[0].cells), (0, 6));
        assert!(layout.backgrounds.is_empty());
    }

    #[test]
    fn a_style_change_starts_a_new_run_and_the_runs_cover_the_text() {
        // "ab" default, "cd" bold red, "ef" default again
        let layout = layout("ab\x1b[1;31mcd\x1b[0mef", 6);
        let segment = &layout.segments[0];
        assert_eq!(segment.text, "abcdef");
        let runs: Vec<(usize, Rgb, bool)> = segment
            .runs
            .iter()
            .map(|(n, s)| (*n, s.fg, s.bold))
            .collect();
        assert_eq!(
            runs,
            [(2, FG, false), (2, palette(1), true), (2, FG, false)]
        );
        assert_eq!(
            segment.runs.iter().map(|r| r.0).sum::<usize>(),
            segment.text.len()
        );
    }

    #[test]
    fn rgb_and_indexed_colours_resolve() {
        let layout = layout("\x1b[38;2;1;2;3mx\x1b[38;5;21my", 2);
        let colours: Vec<Rgb> = layout.segments[0].runs.iter().map(|r| r.1.fg).collect();
        assert_eq!(colours, [Rgb(1, 2, 3), Rgb(0, 0, 255)]);
    }

    #[test]
    fn backgrounds_merge_across_neighbouring_cells_of_the_same_colour() {
        let layout = layout("\x1b[44mab\x1b[41mc\x1b[0m d", 6);
        assert_eq!(
            layout.backgrounds,
            [
                Background {
                    col: 0,
                    cells: 2,
                    color: palette(4)
                },
                Background {
                    col: 2,
                    cells: 1,
                    color: palette(1)
                },
            ]
        );
    }

    #[test]
    fn inverse_swaps_the_colours_using_the_defaults() {
        let layout = layout("\x1b[7mx", 1);
        assert_eq!(
            layout.backgrounds,
            [Background {
                col: 0,
                cells: 1,
                color: FG
            }]
        );
        assert_eq!(layout.segments[0].runs[0].1.fg, BG);
    }

    #[test]
    fn underline_italic_and_dim() {
        let layout = layout("\x1b[3;4mx\x1b[0;2my", 2);
        let runs = &layout.segments[0].runs;
        assert!(runs[0].1.italic && runs[0].1.underline);
        assert_eq!(
            runs[1].1.fg,
            Rgb(0x84, 0x84, 0x84),
            "dim is 60 % of the foreground"
        );
    }

    #[test]
    fn a_wide_character_is_a_segment_of_its_own_two_cells_wide() {
        let layout = layout("a日b", 6);
        let spans: Vec<(usize, usize, &str)> = layout
            .segments
            .iter()
            .map(|s| (s.col, s.cells, s.text.as_str().trim_end()))
            .collect();
        assert_eq!(spans, [(0, 1, "a"), (1, 2, "日"), (3, 3, "b")]);
        assert_eq!(
            layout.segments[1].runs[0].0,
            "日".len(),
            "run lengths are UTF-8 bytes"
        );
    }
}
