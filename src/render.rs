use crate::input::{InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS};
use font8x8::UnicodeFonts;

// ARGB8888 — byte order on disk/in memory is [Blue, Green, Red, Alpha]

// Main grid
const BG: [u8; 4] = [0x00, 0x00, 0x00, 0x00]; // fully transparent
const CELL_NORMAL: [u8; 4] = [0x00, 0x00, 0x00, 0x66]; // 40% dark
const CELL_HIGHLIGHT: [u8; 4] = [0x14, 0x30, 0x14, 0xAA]; // dark green
const CELL_DIM: [u8; 4] = [0x00, 0x00, 0x00, 0x00]; // transparent
const TEXT_FIRST: [u8; 4] = [0x00, 0xDC, 0xFF, 0xFF]; // yellow  (RGB 255,220,0)
const TEXT_SECOND: [u8; 4] = [0xFF, 0xBE, 0x50, 0xFF]; // sky-blue (RGB 80,190,255)
const TEXT_HIGHLIGHT: [u8; 4] = [0x50, 0xFF, 0x50, 0xFF]; // bright lime
const TEXT_DIM: [u8; 4] = [0x66, 0x66, 0x66, 0xAA]; // grey

// Sub-grid (single-keypress horizontal strip inside selected cell)
const SUB_CELL_NORMAL: [u8; 4] = [0x30, 0x10, 0x00, 0xAA]; // dark navy
const SUB_TEXT_FIRST: [u8; 4] = [0x00, 0xDC, 0xFF, 0xFF]; // yellow (same as main)

/// Scale factor for main-grid glyphs (8×FONT_SCALE pixels per glyph).
const FONT_SCALE: u32 = 2;

pub fn render_grid(buf: &mut [u8], w: u32, h: u32, input: &InputState) {
    if let InputState::SubFirst { col, row } = input {
        render_sub_grid(buf, w, h, *col, *row);
        return;
    }

    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&BG);
    }

    let cell_w = w / COLS;
    let cell_h = h / ROWS;

    for row in 0..ROWS {
        for col in 0..COLS {
            let x = col * cell_w;
            let y = row * cell_h;

            let first_hint = HINTS[col as usize];
            let second_hint = HINTS[row as usize];

            let (cell_bg, c1, c2) = match input {
                InputState::First => (CELL_NORMAL, TEXT_FIRST, TEXT_SECOND),
                InputState::Second(typed) => {
                    if first_hint == *typed {
                        (CELL_HIGHLIGHT, TEXT_HIGHLIGHT, TEXT_SECOND)
                    } else {
                        (CELL_DIM, TEXT_DIM, TEXT_DIM)
                    }
                }
                _ => (CELL_DIM, TEXT_DIM, TEXT_DIM),
            };

            fill_rect(buf, w, x + 1, y + 1, cell_w - 2, cell_h - 2, cell_bg);

            let char_w = 8 * FONT_SCALE;
            let char_h = 8 * FONT_SCALE;
            let gap = 3u32;
            let label_w = char_w * 2 + gap;
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
fn render_sub_grid(buf: &mut [u8], w: u32, h: u32, main_col: u32, main_row: u32) {
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&BG);
    }

    let cell_w = w / COLS;
    let cell_h = h / ROWS;
    let cell_x = main_col * cell_w;
    let cell_y = main_row * cell_h;

    // Semi-transparent background so cell borders are visible but content shows through
    const SUB_BG: [u8; 4] = [0x30, 0x10, 0x00, 0x99];
    fill_rect(buf, w, cell_x, cell_y, cell_w, cell_h, SUB_BG);

    // Amber outline around the selected main cell
    let border: [u8; 4] = [0x00, 0xA5, 0xFF, 0xFF];
    fill_rect(buf, w, cell_x, cell_y, cell_w, 1, border);
    fill_rect(buf, w, cell_x, cell_y + cell_h - 1, cell_w, 1, border);
    fill_rect(buf, w, cell_x, cell_y, 1, cell_h, border);
    fill_rect(buf, w, cell_x + cell_w - 1, cell_y, 1, cell_h, border);

    let sub_cell_w = cell_w / SUB_COLS;
    let sub_cell_h = cell_h / SUB_ROWS;

    for sub_row in 0..SUB_ROWS {
        for sub_col in 0..SUB_COLS {
            let x = cell_x + sub_col * sub_cell_w;
            let y = cell_y + sub_row * sub_cell_h;
            let hint = SUB_HINTS[(sub_row * SUB_COLS + sub_col) as usize];

            fill_rect(
                buf,
                w,
                x + 1,
                y + 1,
                sub_cell_w - 2,
                sub_cell_h - 2,
                SUB_CELL_NORMAL,
            );

            // Scale-1 glyphs to fit inside the small sub-cells
            let char_w = 8u32;
            let char_h = 8u32;
            let lx = x + sub_cell_w.saturating_sub(char_w) / 2;
            let ly = y + sub_cell_h.saturating_sub(char_h) / 2;
            draw_glyph(buf, w, lx, ly, hint as char, SUB_TEXT_FIRST, 1);
        }
    }
}

fn fill_rect(buf: &mut [u8], stride: u32, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for dy in 0..h {
        for dx in 0..w {
            let off = ((y + dy) * stride + (x + dx)) as usize * 4;
            if off + 4 <= buf.len() {
                buf[off..off + 4].copy_from_slice(&color);
            }
        }
    }
}

fn draw_glyph(buf: &mut [u8], stride: u32, x: u32, y: u32, ch: char, color: [u8; 4], scale: u32) {
    let glyph = font8x8::BASIC_FONTS.get(ch).unwrap_or([0u8; 8]);
    for (row, &bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            if bits & (1 << col) != 0 {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = x + col * scale + sx;
                        let py = y + row as u32 * scale + sy;
                        let off = (py * stride + px) as usize * 4;
                        if off + 4 <= buf.len() {
                            buf[off..off + 4].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}
