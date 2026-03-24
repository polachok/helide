use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

use crate::renderer::{BgInstance, GlyphAtlas, GlyphInstance};

/// ANSI 256-color palette (first 16 standard terminal colors).
const ANSI_COLORS: [[u8; 3]; 16] = [
    [0, 0, 0],       // Black
    [205, 49, 49],   // Red
    [13, 188, 121],  // Green
    [229, 229, 16],  // Yellow
    [36, 114, 200],  // Blue
    [188, 63, 188],  // Magenta
    [17, 168, 205],  // Cyan
    [229, 229, 229], // White (light gray)
    [102, 102, 102], // Bright black (dark gray)
    [241, 76, 76],   // Bright red
    [35, 209, 139],  // Bright green
    [245, 245, 67],  // Bright yellow
    [59, 142, 234],  // Bright blue
    [214, 112, 214], // Bright magenta
    [41, 184, 219],  // Bright cyan
    [255, 255, 255], // Bright white
];

/// Convert an alacritty_terminal Color to an [f32; 4] RGBA color.
fn alac_color_to_rgba(color: Color, default: [f32; 4]) -> [f32; 4] {
    match color {
        Color::Spec(Rgb { r, g, b }) => {
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        Color::Indexed(n) => indexed_color_to_rgba(n, default),
        Color::Named(named) => named_color_to_rgba(named, default),
    }
}

fn named_color_to_rgba(named: NamedColor, default: [f32; 4]) -> [f32; 4] {
    match named {
        NamedColor::Black => indexed_color_to_rgba(0, default),
        NamedColor::Red => indexed_color_to_rgba(1, default),
        NamedColor::Green => indexed_color_to_rgba(2, default),
        NamedColor::Yellow => indexed_color_to_rgba(3, default),
        NamedColor::Blue => indexed_color_to_rgba(4, default),
        NamedColor::Magenta => indexed_color_to_rgba(5, default),
        NamedColor::Cyan => indexed_color_to_rgba(6, default),
        NamedColor::White => indexed_color_to_rgba(7, default),
        NamedColor::BrightBlack => indexed_color_to_rgba(8, default),
        NamedColor::BrightRed => indexed_color_to_rgba(9, default),
        NamedColor::BrightGreen => indexed_color_to_rgba(10, default),
        NamedColor::BrightYellow => indexed_color_to_rgba(11, default),
        NamedColor::BrightBlue => indexed_color_to_rgba(12, default),
        NamedColor::BrightMagenta => indexed_color_to_rgba(13, default),
        NamedColor::BrightCyan => indexed_color_to_rgba(14, default),
        NamedColor::BrightWhite => indexed_color_to_rgba(15, default),
        NamedColor::Foreground | NamedColor::Background | NamedColor::Cursor => default,
        // Dim variants - use the base color with reduced alpha
        NamedColor::DimBlack => dim_color(indexed_color_to_rgba(0, default)),
        NamedColor::DimRed => dim_color(indexed_color_to_rgba(1, default)),
        NamedColor::DimGreen => dim_color(indexed_color_to_rgba(2, default)),
        NamedColor::DimYellow => dim_color(indexed_color_to_rgba(3, default)),
        NamedColor::DimBlue => dim_color(indexed_color_to_rgba(4, default)),
        NamedColor::DimMagenta => dim_color(indexed_color_to_rgba(5, default)),
        NamedColor::DimCyan => dim_color(indexed_color_to_rgba(6, default)),
        NamedColor::DimWhite => dim_color(indexed_color_to_rgba(7, default)),
        NamedColor::BrightForeground | NamedColor::DimForeground => default,
    }
}

fn dim_color(mut color: [f32; 4]) -> [f32; 4] {
    color[0] *= 0.66;
    color[1] *= 0.66;
    color[2] *= 0.66;
    color
}

fn indexed_color_to_rgba(n: u8, default: [f32; 4]) -> [f32; 4] {
    let (r, g, b) = if n < 16 {
        let c = ANSI_COLORS[n as usize];
        (c[0], c[1], c[2])
    } else if n < 232 {
        let idx = n - 16;
        let b = (idx % 6) * 51;
        let g = ((idx / 6) % 6) * 51;
        let r = (idx / 36) * 51;
        (r, g, b)
    } else if n < 255 {
        let v = 8 + (n - 232) * 10;
        (v, v, v)
    } else {
        return default;
    };
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

/// Build GPU instances from alacritty terminal grid cells.
///
/// Returns (bg_instances, glyph_instances, decoration_instances).
pub fn build_terminal_instances<T: alacritty_terminal::event::EventListener>(
    term: &Term<T>,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
    cell_width: f32,
    cell_height: f32,
    default_fg: [f32; 4],
    default_bg: [f32; 4],
    show_cursor: bool,
    // Multiplier for foreground/decoration colors (1.0 = normal, <1.0 = dimmed).
    fg_dim: f32,
) -> (Vec<BgInstance>, Vec<GlyphInstance>, Vec<BgInstance>) {
    let grid = term.grid();
    let num_lines = grid.screen_lines();
    let num_cols = grid.columns();
    let display_offset = grid.display_offset();

    let mut bg_instances = Vec::with_capacity(num_lines * num_cols);
    let mut glyph_instances = Vec::new();
    let mut decoration_instances = Vec::new();

    for row_idx in 0..num_lines {
        // Adjust line index for scroll offset: negative lines index into history
        let line = Line(row_idx as i32) - display_offset;
        for col_idx in 0..num_cols {
            let cell = &grid[line][Column(col_idx)];
            let ch = cell.c;
            let flags = cell.flags;

            let px = col_idx as f32 * cell_width;
            let py = row_idx as f32 * cell_height;

            // Resolve colors
            let mut fg = alac_color_to_rgba(cell.fg, default_fg);
            let mut bg = alac_color_to_rgba(cell.bg, default_bg);

            // Handle INVERSE flag
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Handle HIDDEN flag
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }

            // Handle DIM flag
            if flags.contains(Flags::DIM) {
                fg[0] *= 0.66;
                fg[1] *= 0.66;
                fg[2] *= 0.66;
            }

            // Dim foreground for inactive pane
            if fg_dim < 1.0 {
                fg[0] *= fg_dim;
                fg[1] *= fg_dim;
                fg[2] *= fg_dim;
            }

            // Skip wide char spacers (second cell of a wide character)
            let is_spacer = flags.contains(Flags::WIDE_CHAR_SPACER);

            // Background
            bg_instances.push(BgInstance {
                pos: [px, py],
                size: [cell_width, cell_height],
                color: bg,
            });

            // Glyph
            if !is_spacer && ch != ' ' && ch != '\0' && !ch.is_control() {
                let bold = flags.contains(Flags::BOLD);
                let italic = flags.contains(Flags::ITALIC);

                let entry = atlas.get_glyph(queue, ch, bold, italic);
                if entry.width > 0.0 {
                    let gx = px + entry.left;
                    let gy = py + atlas.ascent - entry.top;

                    glyph_instances.push(GlyphInstance {
                        pos: [gx, gy],
                        size: [entry.width, entry.height],
                        uv: entry.uv,
                        color: fg,
                    });
                }
            }

            // Underline decorations
            if flags.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) {
                let baseline_y = py + atlas.ascent;
                let thickness = (cell_height / 14.0).max(1.0);

                if flags.contains(Flags::UNDERLINE) {
                    decoration_instances.push(BgInstance {
                        pos: [px, baseline_y + 1.0],
                        size: [cell_width, thickness],
                        color: fg,
                    });
                } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
                    let gap = thickness + 1.0;
                    decoration_instances.push(BgInstance {
                        pos: [px, baseline_y + 1.0],
                        size: [cell_width, thickness],
                        color: fg,
                    });
                    decoration_instances.push(BgInstance {
                        pos: [px, baseline_y + 1.0 + gap],
                        size: [cell_width, thickness],
                        color: fg,
                    });
                } else if flags.contains(Flags::UNDERCURL) {
                    decoration_instances.push(BgInstance {
                        pos: [px, baseline_y + 1.0],
                        size: [cell_width, thickness * 2.0],
                        color: fg,
                    });
                } else if flags.contains(Flags::DOTTED_UNDERLINE) {
                    let dot_w = (cell_width / 4.0).max(2.0);
                    let mut dx = px;
                    while dx < px + cell_width {
                        decoration_instances.push(BgInstance {
                            pos: [dx, baseline_y + 1.0],
                            size: [dot_w * 0.5, thickness],
                            color: fg,
                        });
                        dx += dot_w;
                    }
                } else if flags.contains(Flags::DASHED_UNDERLINE) {
                    let dash_w = (cell_width / 2.0).max(3.0);
                    let gap_w = (cell_width / 4.0).max(2.0);
                    let mut dx = px;
                    while dx < px + cell_width {
                        let w = dash_w.min(px + cell_width - dx);
                        decoration_instances.push(BgInstance {
                            pos: [dx, baseline_y + 1.0],
                            size: [w, thickness],
                            color: fg,
                        });
                        dx += dash_w + gap_w;
                    }
                }
            }

            // Strikethrough
            if flags.contains(Flags::STRIKEOUT) {
                let strike_y = py + cell_height * 0.45;
                let strike_thickness = (cell_height / 14.0).max(1.0);
                decoration_instances.push(BgInstance {
                    pos: [px, strike_y],
                    size: [cell_width, strike_thickness],
                    color: fg,
                });
            }
        }
    }

    // Cursor — only show when not scrolled back
    let cursor = grid.cursor.point;
    let cursor_row = cursor.line.0 as usize;
    let cursor_col = cursor.column.0;
    if show_cursor && display_offset == 0 && cursor_row < num_lines && cursor_col < num_cols {
        let cx = cursor_col as f32 * cell_width;
        let cy = cursor_row as f32 * cell_height;
        // Block cursor: draw a filled rectangle with the foreground color
        decoration_instances.push(BgInstance {
            pos: [cx, cy],
            size: [cell_width, cell_height],
            color: [default_fg[0], default_fg[1], default_fg[2], 0.5],
        });
    }

    (bg_instances, glyph_instances, decoration_instances)
}
