use crate::{
    config::{colors, font_size, panel_font_size, sub_hint_font_size},
    input::{dynamic_cols, dynamic_rows, hints, sub_cols, sub_hints, sub_rows, InputState},
};
use font8x8::UnicodeFonts;

fn line_height(scale: u32) -> u32 {
    8 * scale + 8
}

fn char_width(scale: u32) -> u32 {
    8 * scale
}

fn glyph_scale_for_cell(cell_w: u32, cell_h: u32, requested_scale: u32) -> u32 {
    let fit_w = cell_w.saturating_sub(2) / 8;
    let fit_h = cell_h.saturating_sub(2) / 8;
    requested_scale.min(fit_w.min(fit_h).max(1))
}

fn glyph_bounds(ch: char) -> Option<(u32, u32, u32, u32)> {
    let glyph = font8x8::BASIC_FONTS.get(ch).unwrap_or([0u8; 8]);
    let mut min_x = 8u32;
    let mut max_x = 0u32;
    let mut min_y = 8u32;
    let mut max_y = 0u32;

    for (row, &bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            if bits & (1 << col) != 0 {
                min_x = min_x.min(col);
                max_x = max_x.max(col);
                min_y = min_y.min(row as u32);
                max_y = max_y.max(row as u32);
            }
        }
    }

    (min_x < 8).then_some((min_x, max_x, min_y, max_y))
}

fn glyph_layout(ch: char, cell_w: u32, cell_h: u32, requested_scale: u32) -> (u32, u32, u32) {
    let glyph_scale = glyph_scale_for_cell(cell_w, cell_h, requested_scale);
    if let Some((min_x, max_x, min_y, max_y)) = glyph_bounds(ch) {
        let active_w = (max_x - min_x + 1) * glyph_scale;
        let active_h = (max_y - min_y + 1) * glyph_scale;
        let offset_x = cell_w.saturating_sub(active_w) / 2;
        let offset_y = cell_h.saturating_sub(active_h) / 2;
        return (
            glyph_scale,
            offset_x.saturating_sub(min_x * glyph_scale),
            offset_y.saturating_sub(min_y * glyph_scale),
        );
    }

    let glyph_w = char_width(glyph_scale);
    let glyph_h = 8 * glyph_scale;
    (
        glyph_scale,
        cell_w.saturating_sub(glyph_w) / 2,
        cell_h.saturating_sub(glyph_h) / 2,
    )
}

fn subdivide_span(start: u32, span: u32, index: u32, count: u32) -> (u32, u32) {
    let cell_start = start + index * span / count;
    let cell_end = start + (index + 1) * span / count;
    (cell_start, cell_end.saturating_sub(cell_start))
}

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

    let hints = hints();
    let ncols = dynamic_cols(w);
    let nrows = dynamic_rows(h);
    let cell_w = w / ncols;
    let cell_h = h / nrows;
    let scale = font_size();
    let char_w = 8 * scale;
    let char_h = 8 * scale;
    let gap = 3u32;
    let label_w = char_w * 2 + gap;
    let cell_normal = if dragging {
        colors().cell_drag
    } else {
        colors().cell_normal
    };

    for row in 0..nrows {
        for col in 0..ncols {
            let x = col * cell_w;
            let y = row * cell_h;
            let first_hint = hints[col as usize];
            let second_hint = hints[row as usize];

            let (cell_bg, c1, c2) = match input {
                InputState::First => (Some(cell_normal), colors().text_first, colors().text_second),
                InputState::Second(typed) => {
                    if first_hint == *typed {
                        (
                            Some(colors().cell_highlight),
                            colors().text_highlight,
                            colors().text_second,
                        )
                    } else {
                        (None, colors().text_dim, colors().text_dim)
                    }
                }
                _ => unreachable!(),
            };

            if let Some(bg) = cell_bg {
                c.fill_rect(x + 1, y + 1, cell_w - 2, cell_h - 2, bg);
            }

            let lx = x + cell_w.saturating_sub(label_w) / 2;
            let ly = y + cell_h.saturating_sub(char_h) / 2;
            c.draw_glyph(lx, ly, first_hint, c1, scale);
            c.draw_glyph(lx + char_w + gap, ly, second_hint, c2, scale);
        }
    }
}

pub fn render_rec_indicator(buf: &mut [u8], w: u32) {
    let scale = font_size();
    let mut c = Canvas { buf, w };
    c.fill_rect(8, 8, 8 * scale * 4, line_height(scale), colors().rec_bg);
    c.draw_text(12, 12, b"REC", colors().text_white, scale);
}

pub fn render_macro_bind_key(buf: &mut [u8], w: u32, h: u32) {
    let mut p = Panel::new(buf, w, h, 6);
    p.text(b"save macro", colors().text_first)
        .skip()
        .text(b"press a key to bind", colors().text_white)
        .text(b"enter to skip binding", colors().text_grey)
        .text(b"escape to cancel", colors().text_grey);
}

pub fn render_macro_name(buf: &mut [u8], w: u32, h: u32, name: &[char], bind_key: Option<char>) {
    let mut p = Panel::new(buf, w, h, 7);
    p.text(b"name this macro", colors().text_first);
    match bind_key {
        Some(k) => p.text_with_char(b"bound to ", k, colors().text_grey),
        None => p.skip(),
    };
    p.input_line(name, colors().text_white)
        .skip()
        .text(b"enter to save", colors().text_grey)
        .text(b"escape to cancel", colors().text_grey);
}

pub fn render_macro_replay_wait(buf: &mut [u8], w: u32, h: u32) {
    let mut p = Panel::new(buf, w, h, 4);
    p.text(b"press macro key", colors().text_first)
        .skip()
        .text(b"escape to cancel", colors().text_grey);
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
    p.input_line(query, colors().text_white).skip();
    if results.is_empty() {
        p.text(b"no results", colors().text_grey);
    } else {
        for (i, (bind_key, name)) in results[..visible].iter().enumerate() {
            p.search_entry(*bind_key, name, i == selected);
        }
    }
    p.skip()
        .text(b"tab:next enter:select esc:back", colors().text_grey);
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
/// breathing room; `panel_h = rows * line_height(scale) + 32` (clamped to screen height).
struct Panel<'a> {
    c: Canvas<'a>,
    tx: u32, // left edge of text column
    px: u32, // left edge of panel (for row highlights)
    pw: u32, // panel width (for row highlights)
    ty: u32, // current y cursor
    scale: u32,
}

impl<'a> Panel<'a> {
    fn new(buf: &'a mut [u8], w: u32, h: u32, rows: u32) -> Self {
        let mut c = Canvas { buf, w };
        c.clear();
        let scale = panel_font_size();
        let lh = line_height(scale);
        let panel_h = (rows * lh + 32).min(h.saturating_sub(4));
        let min_panel_chars = 30;
        let panel_padding = 16 * scale;
        let panel_min_w = min_panel_chars * char_width(scale) + panel_padding * 2;
        let panel_w = (w * 30 / 100).max(panel_min_w).min(w);
        let panel_x = (w.saturating_sub(panel_w)) / 2;
        let panel_y = (h.saturating_sub(panel_h)) / 2;
        c.fill_rect(panel_x, panel_y, panel_w, panel_h, colors().panel_bg);
        Self {
            c,
            tx: panel_x + panel_padding,
            px: panel_x,
            pw: panel_w,
            ty: panel_y + panel_padding,
            scale,
        }
    }

    fn text(&mut self, text: &[u8], color: [u8; 4]) -> &mut Self {
        self.c.draw_text(self.tx, self.ty, text, color, self.scale);
        self.ty += line_height(self.scale);
        self
    }

    fn skip(&mut self) -> &mut Self {
        self.ty += line_height(self.scale);
        self
    }

    fn text_with_char(&mut self, label: &[u8], ch: char, color: [u8; 4]) -> &mut Self {
        self.c.draw_text(self.tx, self.ty, label, color, self.scale);
        self.c.draw_glyph(
            self.tx + label.len() as u32 * char_width(self.scale),
            self.ty,
            ch,
            color,
            self.scale,
        );
        self.ty += line_height(self.scale);
        self
    }

    /// Draws a `> chars_` text-input prompt line.
    fn input_line(&mut self, chars: &[char], color: [u8; 4]) -> &mut Self {
        let cw = char_width(self.scale);
        self.c.draw_text(self.tx, self.ty, b"> ", color, self.scale);
        self.c
            .draw_chars(self.tx + 2 * cw, self.ty, chars, color, self.scale);
        self.c.draw_glyph(
            self.tx + (2 + chars.len() as u32) * cw,
            self.ty,
            '_',
            color,
            self.scale,
        );
        self.ty += line_height(self.scale);
        self
    }

    fn search_entry(&mut self, bind_key: Option<char>, name: &str, selected: bool) -> &mut Self {
        let cw = char_width(self.scale);
        let lh = line_height(self.scale);
        if selected {
            self.c.fill_rect(
                self.px + 4,
                self.ty.saturating_sub(2),
                self.pw - 8,
                lh,
                colors().selected_bg,
            );
        }
        let text_color = if selected {
            colors().text_highlight
        } else {
            colors().text_white
        };
        match bind_key {
            Some(k) => {
                self.c
                    .draw_text(self.tx, self.ty, b"[", colors().text_grey, self.scale);
                self.c
                    .draw_glyph(self.tx + cw, self.ty, k, colors().text_grey, self.scale);
                self.c.draw_text(
                    self.tx + 2 * cw,
                    self.ty,
                    b"] ",
                    colors().text_grey,
                    self.scale,
                );
            }
            None => self
                .c
                .draw_text(self.tx, self.ty, b"[ ] ", colors().text_grey, self.scale),
        }
        self.c.draw_text(
            self.tx + 4 * cw,
            self.ty,
            name.as_bytes(),
            text_color,
            self.scale,
        );
        self.ty += line_height(self.scale);
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
    let nsub_cols = sub_cols();
    let nsub_rows = sub_rows();
    let sub_hints = sub_hints();
    let ncols = dynamic_cols(c.w);
    let nrows = dynamic_rows(h);

    // Early return if main_col/main_row are outside the rendered grid
    if main_col >= ncols || main_row >= nrows {
        return;
    }

    let cell_w = c.w / ncols;
    let cell_h = h / nrows;
    let cell_x = main_col * cell_w;
    let cell_y = main_row * cell_h;

    c.fill_rect(cell_x, cell_y, cell_w, cell_h, colors().sub_bg);

    let border = if dragging {
        colors().border_dragging
    } else {
        colors().border
    };
    c.fill_rect(cell_x, cell_y, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y + cell_h - 1, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y, 1, cell_h, border);
    c.fill_rect(cell_x + cell_w - 1, cell_y, 1, cell_h, border);

    let sub_hint_scale = sub_hint_font_size();
    let col_spans: Vec<_> = (0..nsub_cols)
        .map(|sub_col| subdivide_span(cell_x, cell_w, sub_col, nsub_cols))
        .collect();
    let row_spans: Vec<_> = (0..nsub_rows)
        .map(|sub_row| subdivide_span(cell_y, cell_h, sub_row, nsub_rows))
        .collect();
    for sub_row in 0..nsub_rows {
        for sub_col in 0..nsub_cols {
            let (x, sub_cell_w) = col_spans[sub_col as usize];
            let (y, sub_cell_h) = row_spans[sub_row as usize];
            let hint = sub_hints[(sub_row * nsub_cols + sub_col) as usize];
            let (glyph_scale, glyph_ox, glyph_oy) =
                glyph_layout(hint, sub_cell_w, sub_cell_h, sub_hint_scale);
            let is_selected = selected == Some((sub_col, sub_row));
            let (bg, text) = if is_selected {
                (colors().cell_highlight, colors().text_highlight)
            } else {
                (colors().sub_cell_normal, colors().text_first)
            };
            c.fill_rect(
                x + 1,
                y + 1,
                sub_cell_w.saturating_sub(2),
                sub_cell_h.saturating_sub(2),
                bg,
            );
            c.draw_glyph(x + glyph_ox, y + glyph_oy, hint, text, glyph_scale);
        }
    }
}
