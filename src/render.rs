use crate::input::{InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS};
use font8x8::UnicodeFonts;

// ARGB8888 — byte order on disk/in memory is [Blue, Green, Red, Alpha]

// Main grid
const CELL_NORMAL: [u8; 4] = [0x00, 0x00, 0x00, 0x66]; // 40% dark
const CELL_DRAG: [u8; 4] = [0x40, 0x00, 0x40, 0x88]; // dark purple (drag mode)
const CELL_HIGHLIGHT: [u8; 4] = [0x14, 0x30, 0x14, 0xAA]; // dark green
const TEXT_FIRST: [u8; 4] = [0x00, 0xDC, 0xFF, 0xFF]; // yellow  (RGB 255,220,0)
const TEXT_SECOND: [u8; 4] = [0xFF, 0xBE, 0x50, 0xFF]; // sky-blue (RGB 80,190,255)
const TEXT_HIGHLIGHT: [u8; 4] = [0x50, 0xFF, 0x50, 0xFF]; // bright lime
const TEXT_DIM: [u8; 4] = [0x66, 0x66, 0x66, 0xAA]; // grey

// Sub-grid (single-keypress horizontal strip inside selected cell)
const SUB_CELL_NORMAL: [u8; 4] = [0x30, 0x10, 0x00, 0xAA]; // dark navy

/// Scale factor for main-grid glyphs (8×FONT_SCALE pixels per glyph).
const FONT_SCALE: u32 = 2;

pub fn render_grid(buf: &mut [u8], w: u32, h: u32, input: &InputState, dragging: bool) {
    match input {
        InputState::SubFirst { col, row } => {
            render_sub_grid(buf, w, h, *col, *row, None, dragging);
            return;
        }
        InputState::Ready {
            col,
            row,
            sub_col,
            sub_row,
        } => {
            render_sub_grid(buf, w, h, *col, *row, Some((*sub_col, *sub_row)), dragging);
            return;
        }
        _ => {}
    }

    let cell_w = w / COLS;
    let cell_h = h / ROWS;
    let char_w = 8 * FONT_SCALE;
    let char_h = 8 * FONT_SCALE;
    let gap = 3u32;
    let label_w = char_w * 2 + gap;
    let cell_normal = if dragging { CELL_DRAG } else { CELL_NORMAL };

    for row in 0..ROWS {
        for col in 0..COLS {
            let x = col * cell_w;
            let y = row * cell_h;

            let first_hint = HINTS[col as usize];
            let second_hint = HINTS[row as usize];

            let (cell_bg, c1, c2) = match input {
                InputState::First => (Some(cell_normal), TEXT_FIRST, TEXT_SECOND),
                InputState::Second(typed) => {
                    if first_hint == *typed {
                        (Some(CELL_HIGHLIGHT), TEXT_HIGHLIGHT, TEXT_SECOND)
                    } else {
                        (None, TEXT_DIM, TEXT_DIM)
                    }
                }
                _ => unreachable!(),
            };

            if let Some(bg) = cell_bg {
                fill_rect(buf, w, x + 1, y + 1, cell_w - 2, cell_h - 2, bg);
            }

            let lx = x + cell_w.saturating_sub(label_w) / 2;
            let ly = y + cell_h.saturating_sub(char_h) / 2;

            draw_glyph(buf, w, lx, ly, first_hint as char, c1, FONT_SCALE);
            draw_glyph(
                buf,
                w,
                lx + char_w + gap,
                ly,
                second_hint as char,
                c2,
                FONT_SCALE,
            );
        }
    }
}

/// Renders a 5×5 sub-grid inside the selected main cell.
/// Each of the 25 cells has a unique single char — one keypress selects it.
fn render_sub_grid(
    buf: &mut [u8],
    w: u32,
    h: u32,
    main_col: u32,
    main_row: u32,
    selected: Option<(u32, u32)>,
    dragging: bool,
) {
    buf.fill(0);

    let cell_w = w / COLS;
    let cell_h = h / ROWS;
    let cell_x = main_col * cell_w;
    let cell_y = main_row * cell_h;

    const SUB_BG: [u8; 4] = [0x30, 0x10, 0x00, 0x99];
    fill_rect(buf, w, cell_x, cell_y, cell_w, cell_h, SUB_BG);

    let border: [u8; 4] = if dragging {
        [0xFF, 0x00, 0xFF, 0xFF] // magenta
    } else {
        [0x00, 0xA5, 0xFF, 0xFF] // amber
    };
    fill_rect(buf, w, cell_x, cell_y, cell_w, 1, border);
    fill_rect(buf, w, cell_x, cell_y + cell_h - 1, cell_w, 1, border);
    fill_rect(buf, w, cell_x, cell_y, 1, cell_h, border);
    fill_rect(buf, w, cell_x + cell_w - 1, cell_y, 1, cell_h, border);

    let sub_cell_w = cell_w / SUB_COLS;
    let sub_cell_h = cell_h / SUB_ROWS;
    let glyph_ox = sub_cell_w.saturating_sub(8) / 2;
    let glyph_oy = sub_cell_h.saturating_sub(8) / 2;

    for sub_row in 0..SUB_ROWS {
        for sub_col in 0..SUB_COLS {
            let x = cell_x + sub_col * sub_cell_w;
            let y = cell_y + sub_row * sub_cell_h;
            let hint = SUB_HINTS[(sub_row * SUB_COLS + sub_col) as usize];

            let is_selected = selected == Some((sub_col, sub_row));
            let (bg, text) = if is_selected {
                (CELL_HIGHLIGHT, TEXT_HIGHLIGHT)
            } else {
                (SUB_CELL_NORMAL, TEXT_FIRST)
            };

            fill_rect(buf, w, x + 1, y + 1, sub_cell_w - 2, sub_cell_h - 2, bg);
            draw_glyph(buf, w, x + glyph_ox, y + glyph_oy, hint as char, text, 1);
        }
    }
}

fn fill_rect(buf: &mut [u8], stride: u32, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for dy in 0..h {
        let row_start = ((y + dy) * stride + x) as usize * 4;
        let row_end = row_start + w as usize * 4;
        if row_end <= buf.len() {
            for px in buf[row_start..row_end].chunks_exact_mut(4) {
                px.copy_from_slice(&color);
            }
        }
    }
}

fn draw_glyph(buf: &mut [u8], stride: u32, x: u32, y: u32, ch: char, color: [u8; 4], scale: u32) {
    let glyph = font8x8::BASIC_FONTS.get(ch).unwrap_or([0u8; 8]);
    for (row, &bits) in glyph.iter().enumerate() {
        for sy in 0..scale {
            let py = y + row as u32 * scale + sy;
            let row_off = (py * stride) as usize * 4;
            if row_off + ((x + 8 * scale) as usize) * 4 <= buf.len() {
                for col in 0..8u32 {
                    if bits & (1 << col) != 0 {
                        for sx in 0..scale {
                            let off = row_off + (x + col * scale + sx) as usize * 4;
                            buf[off..off + 4].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}
