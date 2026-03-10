use crate::input::{InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS};
use font8x8::UnicodeFonts;

// ARGB8888 — byte order on disk/in memory is [Blue, Green, Red, Alpha]

// Main grid
const CELL_NORMAL: [u8; 4] = [0x00, 0x00, 0x00, 0x66];
const CELL_DRAG: [u8; 4] = [0x40, 0x00, 0x40, 0x88]; // dark purple
const CELL_HIGHLIGHT: [u8; 4] = [0x14, 0x30, 0x14, 0xAA]; // dark green
const TEXT_FIRST: [u8; 4] = [0x00, 0xDC, 0xFF, 0xFF]; // yellow  (RGB 255,220,0)
const TEXT_SECOND: [u8; 4] = [0xFF, 0xBE, 0x50, 0xFF]; // sky-blue (RGB 80,190,255)
const TEXT_HIGHLIGHT: [u8; 4] = [0x50, 0xFF, 0x50, 0xFF]; // bright lime
const TEXT_DIM: [u8; 4] = [0x66, 0x66, 0x66, 0xAA];

// Sub-grid
const SUB_CELL_NORMAL: [u8; 4] = [0x30, 0x10, 0x00, 0xAA]; // dark navy

// Macro UI
const PANEL_BG: [u8; 4] = [0x18, 0x0C, 0x00, 0xEE]; // very dark, warm
const TEXT_WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const TEXT_GREY: [u8; 4] = [0x88, 0x88, 0x88, 0xFF];
const SELECTED_BG: [u8; 4] = [0x30, 0x50, 0x20, 0xFF]; // dark green
const REC_BG: [u8; 4] = [0x00, 0x00, 0xCC, 0xFF]; // red

const FONT_SCALE: u32 = 2;
const LINE_H: u32 = 24;

pub fn render_grid(buf: &mut [u8], w: u32, h: u32, input: &InputState, dragging: bool) {
    let mut c = Canvas { buf, w };
    c.clear();
    match input {
        InputState::SubFirst { col, row } => {
            render_sub_grid(&mut c, h, *col, *row, None, dragging);
            return;
        }
        InputState::Ready {
            col,
            row,
            sub_col,
            sub_row,
        } => {
            render_sub_grid(&mut c, h, *col, *row, Some((*sub_col, *sub_row)), dragging);
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
                c.fill_rect(x + 1, y + 1, cell_w - 2, cell_h - 2, bg);
            }

            let lx = x + cell_w.saturating_sub(label_w) / 2;
            let ly = y + cell_h.saturating_sub(char_h) / 2;
            c.draw_glyph(lx, ly, first_hint, c1, FONT_SCALE);
            c.draw_glyph(lx + char_w + gap, ly, second_hint, c2, FONT_SCALE);
        }
    }
}

pub fn render_rec_indicator(buf: &mut [u8], w: u32) {
    let mut c = Canvas { buf, w };
    c.fill_rect(8, 8, 56, 24, REC_BG);
    c.draw_text(12, 12, b"REC", TEXT_WHITE, 2);
}

pub fn render_macro_bind_key(buf: &mut [u8], w: u32, h: u32) {
    let mut p = Panel::new(buf, w, h, 6);
    p.text(b"save macro", TEXT_FIRST)
        .skip()
        .text(b"press a key to bind", TEXT_WHITE)
        .text(b"enter to skip binding", TEXT_GREY)
        .text(b"escape to cancel", TEXT_GREY);
}

pub fn render_macro_name(buf: &mut [u8], w: u32, h: u32, name: &[char], bind_key: Option<char>) {
    let mut p = Panel::new(buf, w, h, 7);
    p.text(b"name this macro", TEXT_FIRST);
    match bind_key {
        Some(k) => p.text_with_char(b"bound to ", k, TEXT_GREY),
        None => p.skip(),
    };
    p.input_line(name, TEXT_WHITE)
        .skip()
        .text(b"enter to save", TEXT_GREY)
        .text(b"escape to cancel", TEXT_GREY);
}

pub fn render_macro_replay_wait(buf: &mut [u8], w: u32, h: u32) {
    let mut p = Panel::new(buf, w, h, 4);
    p.text(b"press macro key", TEXT_FIRST)
        .skip()
        .text(b"escape to cancel", TEXT_GREY);
}

pub fn render_macro_search(
    buf: &mut [u8],
    w: u32,
    h: u32,
    query: &[char],
    results: &[(Option<char>, &str)],
    selected: usize,
) {
    let max_visible = 10usize;
    let visible = results.len().min(max_visible);
    let mut p = Panel::new(buf, w, h, visible as u32 + 5);
    p.input_line(query, TEXT_WHITE).skip();
    if results.is_empty() {
        p.text(b"no results", TEXT_GREY);
    } else {
        for (i, (bind_key, name)) in results[..visible].iter().enumerate() {
            p.search_entry(*bind_key, name, i == selected);
        }
    }
    p.skip().text(b"tab:next enter:select esc:back", TEXT_GREY);
}

struct Canvas<'a> {
    buf: &'a mut [u8],
    w: u32,
}

impl<'a> Canvas<'a> {
    fn clear(&mut self) {
        self.buf.fill(0);
    }

    fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
        for dy in 0..h {
            let row_start = ((y + dy) * self.w + x) as usize * 4;
            let row_end = row_start + w as usize * 4;
            if row_end <= self.buf.len() {
                for px in self.buf[row_start..row_end].chunks_exact_mut(4) {
                    px.copy_from_slice(&color);
                }
            }
        }
    }

    fn draw_glyph(&mut self, x: u32, y: u32, ch: char, color: [u8; 4], scale: u32) {
        let glyph = font8x8::BASIC_FONTS.get(ch).unwrap_or([0u8; 8]);
        let x_end_bytes = (x + 8 * scale) as usize * 4;
        for (row, &bits) in glyph.iter().enumerate() {
            for sy in 0..scale {
                let py = y + row as u32 * scale + sy;
                let row_off = (py * self.w) as usize * 4;
                if row_off + x_end_bytes <= self.buf.len() {
                    for col in 0..8u32 {
                        if bits & (1 << col) != 0 {
                            for sx in 0..scale {
                                let off = row_off + (x + col * scale + sx) as usize * 4;
                                self.buf[off..off + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
    }

    fn draw_text(&mut self, x: u32, y: u32, text: &[u8], color: [u8; 4], scale: u32) {
        for (i, &ch) in text.iter().enumerate() {
            self.draw_glyph(x + i as u32 * 8 * scale, y, ch as char, color, scale);
        }
    }

    fn draw_chars(&mut self, x: u32, y: u32, chars: &[char], color: [u8; 4], scale: u32) {
        for (i, &ch) in chars.iter().enumerate() {
            self.draw_glyph(x + i as u32 * 8 * scale, y, ch, color, scale);
        }
    }
}

/// Wraps a Canvas with layout tracking for centered popup panels.
/// `rows` is the number of line-slots the content uses plus one for bottom
/// breathing room; `panel_h = rows * LINE_H + 32`.
struct Panel<'a> {
    c: Canvas<'a>,
    tx: u32, // left edge of text column
    px: u32, // left edge of panel (for row highlights)
    pw: u32, // panel width (for row highlights)
    ty: u32, // current y cursor
}

impl<'a> Panel<'a> {
    fn new(buf: &'a mut [u8], w: u32, h: u32, rows: u32) -> Self {
        let mut c = Canvas { buf, w };
        c.clear();
        let panel_h = rows * LINE_H + 32;
        let panel_w = (w * 30 / 100).max(400).min(w);
        let panel_x = (w - panel_w) / 2;
        let panel_y = (h - panel_h) / 2;
        c.fill_rect(panel_x, panel_y, panel_w, panel_h, PANEL_BG);
        Self {
            c,
            tx: panel_x + 16,
            px: panel_x,
            pw: panel_w,
            ty: panel_y + 16,
        }
    }

    fn text(&mut self, text: &[u8], color: [u8; 4]) -> &mut Self {
        self.c.draw_text(self.tx, self.ty, text, color, 2);
        self.ty += LINE_H;
        self
    }

    fn skip(&mut self) -> &mut Self {
        self.ty += LINE_H;
        self
    }

    fn text_with_char(&mut self, label: &[u8], ch: char, color: [u8; 4]) -> &mut Self {
        self.c.draw_text(self.tx, self.ty, label, color, 2);
        self.c
            .draw_glyph(self.tx + label.len() as u32 * 16, self.ty, ch, color, 2);
        self.ty += LINE_H;
        self
    }

    /// Draws a `> chars_` text-input prompt line.
    fn input_line(&mut self, chars: &[char], color: [u8; 4]) -> &mut Self {
        self.c.draw_text(self.tx, self.ty, b"> ", color, 2);
        self.c
            .draw_chars(self.tx + 2 * 16, self.ty, chars, color, 2);
        self.c.draw_glyph(
            self.tx + (2 + chars.len() as u32) * 16,
            self.ty,
            '_',
            color,
            2,
        );
        self.ty += LINE_H;
        self
    }

    fn search_entry(&mut self, bind_key: Option<char>, name: &str, selected: bool) -> &mut Self {
        if selected {
            self.c.fill_rect(
                self.px + 4,
                self.ty.saturating_sub(2),
                self.pw - 8,
                LINE_H,
                SELECTED_BG,
            );
        }
        let text_color = if selected { TEXT_HIGHLIGHT } else { TEXT_WHITE };
        match bind_key {
            Some(k) => {
                self.c.draw_text(self.tx, self.ty, b"[", TEXT_GREY, 2);
                self.c.draw_glyph(self.tx + 16, self.ty, k, TEXT_GREY, 2);
                self.c
                    .draw_text(self.tx + 2 * 16, self.ty, b"] ", TEXT_GREY, 2);
            }
            None => self.c.draw_text(self.tx, self.ty, b"[ ] ", TEXT_GREY, 2),
        }
        self.c
            .draw_text(self.tx + 4 * 16, self.ty, name.as_bytes(), text_color, 2);
        self.ty += LINE_H;
        self
    }
}

fn render_sub_grid(
    c: &mut Canvas<'_>,
    h: u32,
    main_col: u32,
    main_row: u32,
    selected: Option<(u32, u32)>,
    dragging: bool,
) {
    let cell_w = c.w / COLS;
    let cell_h = h / ROWS;
    let cell_x = main_col * cell_w;
    let cell_y = main_row * cell_h;

    const SUB_BG: [u8; 4] = [0x30, 0x10, 0x00, 0x99];
    c.fill_rect(cell_x, cell_y, cell_w, cell_h, SUB_BG);

    let border = if dragging {
        [0xFF, 0x00, 0xFF, 0xFF_u8] // magenta
    } else {
        [0x00, 0xA5, 0xFF, 0xFF_u8] // amber
    };
    c.fill_rect(cell_x, cell_y, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y + cell_h - 1, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y, 1, cell_h, border);
    c.fill_rect(cell_x + cell_w - 1, cell_y, 1, cell_h, border);

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
            c.fill_rect(x + 1, y + 1, sub_cell_w - 2, sub_cell_h - 2, bg);
            c.draw_glyph(x + glyph_ox, y + glyph_oy, hint, text, 1);
        }
    }
}
