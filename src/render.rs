use crate::{
    config::config,
    hint::{label_starts_with, HintElement},
    input::InputState,
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
    let cfg = config();
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

    let hints = cfg.hints();
    let ncols = cfg.dynamic_cols(w);
    let nrows = cfg.dynamic_rows(h);
    let cell_w = w / ncols;
    let cell_h = h / nrows;
    let scale = cfg.font_size();
    let char_w = 8 * scale;
    let char_h = 8 * scale;
    let gap = 3u32;
    let label_w = char_w * 2 + gap;
    let colors = &cfg.colors;
    let cell_normal = if dragging {
        colors.cell_drag
    } else {
        colors.cell_normal
    };

    for row in 0..nrows {
        for col in 0..ncols {
            let x = col * cell_w;
            let y = row * cell_h;
            let first_hint = hints[col as usize];
            let second_hint = hints[row as usize];

            let (cell_bg, c1, c2) = match input {
                InputState::First => (Some(cell_normal), colors.text_first, colors.text_second),
                InputState::Second(typed) => {
                    if first_hint == *typed {
                        (
                            Some(colors.cell_highlight),
                            colors.text_highlight,
                            colors.text_second,
                        )
                    } else {
                        (None, colors.text_dim, colors.text_dim)
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

    // Inner grid lines only (1..ncols / 1..nrows skips the outer edge).
    // Transparent by default, so the look is unchanged unless set.
    let separator = colors.separator.unwrap_or([0, 0, 0, 0]);
    if separator[3] != 0 {
        for col in 1..ncols {
            c.fill_rect(col * cell_w, 0, 1, nrows * cell_h, separator);
        }
        for row in 1..nrows {
            c.fill_rect(0, row * cell_h, ncols * cell_w, 1, separator);
        }
    }
}

pub fn render_bisect(buf: &mut [u8], w: u32, h: u32, region: (u32, u32, u32, u32)) {
    let cfg = config();
    let bcfg = &cfg.bisect;
    let rows = bcfg.rows.max(1);
    let cols = bcfg.cols.max(1);
    let (rx, ry, rw, rh) = region;
    let colors = &cfg.colors;
    let mut c = Canvas { buf, w };
    c.clear();

    let rx = rx.min(w);
    let ry = ry.min(h);
    let rw = rw.min(w.saturating_sub(rx));
    let rh = rh.min(h.saturating_sub(ry));
    if rw == 0 || rh == 0 {
        return;
    }

    let sub_w_approx = rw / cols;
    let sub_h_approx = rh / rows;
    let can_split = sub_w_approx >= bcfg.min_cell_size && sub_h_approx >= bcfg.min_cell_size;

    if !can_split {
        c.fill_rect(rx, ry, rw, rh, colors.cell_highlight);
        c.fill_rect(rx, ry, rw, 1, colors.border);
        c.fill_rect(rx, ry + rh - 1, rw, 1, colors.border);
        c.fill_rect(rx, ry, 1, rh, colors.border);
        c.fill_rect(rx + rw - 1, ry, 1, rh, colors.border);
        return;
    }

    let scale = cfg.font_size();
    let col_spans: Vec<_> = (0..cols)
        .map(|col| subdivide_span(rx, rw, col, cols))
        .collect();
    let row_spans: Vec<_> = (0..rows)
        .map(|row| subdivide_span(ry, rh, row, rows))
        .collect();

    for row in 0..rows {
        for col in 0..cols {
            let (x, sub_w) = col_spans[col as usize];
            let (y, sub_h) = row_spans[row as usize];
            let Some(&hint) = bcfg.hints.get((row * cols + col) as usize) else {
                continue;
            };
            c.fill_rect(
                x + 1,
                y + 1,
                sub_w.saturating_sub(2),
                sub_h.saturating_sub(2),
                colors.cell_normal,
            );
            let (gs, ox, oy) = glyph_layout(hint, sub_w, sub_h, scale);
            c.draw_glyph(x + ox, y + oy, hint, colors.text_first, gs);
        }
    }

    // Inner separators between bisect cells (at the sub-span boundaries,
    // skipping the region edges). Transparent by default.
    let separator = colors.separator_bisect.unwrap_or([0, 0, 0, 0]);
    if separator[3] != 0 {
        for col in 1..cols {
            let (x, _) = col_spans[col as usize];
            c.fill_rect(x, ry, 1, rh, separator);
        }
        for row in 1..rows {
            let (y, _) = row_spans[row as usize];
            c.fill_rect(rx, y, rw, 1, separator);
        }
    }

    c.fill_rect(rx, ry, rw, 1, colors.border);
    c.fill_rect(rx, ry + rh - 1, rw, 1, colors.border);
    c.fill_rect(rx, ry, 1, rh, colors.border);
    c.fill_rect(rx + rw - 1, ry, 1, rh, colors.border);
}

pub fn render_rec_indicator(buf: &mut [u8], w: u32) {
    let cfg = config();
    let scale = cfg.font_size();
    let colors = &cfg.colors;
    let mut c = Canvas { buf, w };
    c.fill_rect(8, 8, 8 * scale * 4, line_height(scale), colors.rec_bg);
    c.draw_text(12, 12, b"REC", colors.text_white, scale);
}

pub fn render_free(buf: &mut [u8], w: u32, h: u32, x: u32, y: u32, speed: u32) {
    let cfg = config();
    let colors = &cfg.colors;
    let mut c = Canvas { buf, w };
    c.clear();

    // Crosshair centered on the cursor position.
    let arm = 14u32;
    let thickness = 2u32;
    let gap = 4u32;
    let hbar_y = y.saturating_sub(thickness / 2);
    let vbar_x = x.saturating_sub(thickness / 2);
    // Left arm
    let left_end = x.saturating_sub(gap);
    let left_start = left_end.saturating_sub(arm);
    c.fill_rect(
        left_start,
        hbar_y,
        left_end - left_start,
        thickness,
        colors.crosshair,
    );
    // Right arm
    let right_start = (x + gap).min(w);
    let right_end = (right_start + arm).min(w);
    c.fill_rect(
        right_start,
        hbar_y,
        right_end - right_start,
        thickness,
        colors.crosshair,
    );
    // Top arm
    let top_end = y.saturating_sub(gap);
    let top_start = top_end.saturating_sub(arm);
    c.fill_rect(
        vbar_x,
        top_start,
        thickness,
        top_end - top_start,
        colors.crosshair,
    );
    // Bottom arm
    let bot_start = (y + gap).min(h);
    let bot_end = (bot_start + arm).min(h);
    c.fill_rect(
        vbar_x,
        bot_start,
        thickness,
        bot_end - bot_start,
        colors.crosshair,
    );

    let scale = cfg.font_size();
    let label = format!("FREE speed:{speed}");
    let bytes = label.as_bytes();
    let pad = 2 * scale;
    let lh = line_height(scale);
    let panel_w = bytes.len() as u32 * char_width(scale) + pad * 2;
    c.fill_rect(8, 8, panel_w, lh, colors.panel_bg);
    c.draw_text(8 + pad, 12, bytes, colors.text_white, scale);
}

pub fn render_hints(buf: &mut [u8], w: u32, h: u32, elements: &[HintElement], typed: &[char]) {
    let cfg = config();
    let colors = &cfg.colors;
    let scale = cfg.hint_font_size();
    let cw = char_width(scale);
    let chip_h = 8 * scale + 6;
    let mut c = Canvas { buf, w };
    c.clear();

    if elements.is_empty() {
        c.draw_text(
            8,
            8,
            b"no clickable elements found - esc to exit",
            colors.hint_text,
            scale,
        );
        return;
    }

    // Place every chip so chips never cover each other's letters. `elements` is
    // sorted by importance, so the most important targets keep their natural spot
    // and overlapping neighbours (e.g. a browser tab and its close button) are
    // nudged to the nearest free position instead of stacking unreadably.
    let placements = place_chips(elements, w, h, chip_h, cw);

    // Layered draw: dimmed (non-matching) chips first, then the matching ones on
    // top, so a label you can still type is never hidden under a dimmed
    // neighbour. Reverse order within each layer keeps important chips on top.
    for matching in [false, true] {
        for i in (0..elements.len()).rev() {
            if label_starts_with(&elements[i].label, typed) != matching {
                continue;
            }
            let (x, y, chip_w) = placements[i];
            draw_hint_chip(
                &mut c,
                colors,
                scale,
                cw,
                chip_h,
                &elements[i],
                x,
                y,
                chip_w,
                typed,
                matching,
            );
        }
    }

    // Crosshair on the fully-typed target, drawn last so it sits above all chips.
    if !typed.is_empty() {
        for el in elements {
            if el.label.chars().eq(typed.iter().copied()) {
                draw_crosshair(&mut c, colors, el.cx, el.cy);
            }
        }
    }
}

/// Greedily assign each element a chip rectangle `(x, y, chip_w)` that does not
/// overlap an already-placed chip, searching positions near the element. Earlier
/// (more important) elements are placed first and win contested spots.
fn place_chips(
    elements: &[HintElement],
    w: u32,
    h: u32,
    chip_h: u32,
    cw: u32,
) -> Vec<(u32, u32, u32)> {
    let mut placed: Vec<(u32, u32, u32, u32)> = Vec::with_capacity(elements.len());
    let mut out = Vec::with_capacity(elements.len());
    for el in elements {
        let chip_w = (el.label.chars().count() as u32 * cw + 8).max(18);
        let rect = find_free_chip(el.bbox, chip_w, chip_h, w, h, &placed);
        placed.push(rect);
        out.push((rect.0, rect.1, chip_w));
    }
    out
}

/// Search positions anchored around an element for one whose chip rect clears all
/// `placed` rects (plus a small gap). Falls back to the preferred spot, accepting
/// overlap, when the area is too crowded to find a free slot.
fn find_free_chip(
    bbox: (u32, u32, u32, u32),
    chip_w: u32,
    chip_h: u32,
    w: u32,
    h: u32,
    placed: &[(u32, u32, u32, u32)],
) -> (u32, u32, u32, u32) {
    let (bx, by, bw, bh) = bbox;
    let clamp_x = |x: i64| x.clamp(0, w.saturating_sub(chip_w) as i64) as u32;
    let clamp_y = |y: i64| y.clamp(0, h.saturating_sub(chip_h) as i64) as u32;
    let left = clamp_x(bx as i64);
    let right = clamp_x(bx as i64 + bw as i64 - chip_w as i64);
    let on = clamp_y(by as i64 - chip_h as i64 / 2);
    let above = clamp_y(by as i64 - chip_h as i64);
    let below = clamp_y(by as i64 + bh as i64);

    let mut candidates: Vec<(u32, u32)> = vec![
        (left, on),
        (left, above),
        (right, above),
        (right, on),
        (left, below),
        (right, below),
    ];
    // Then fan out vertically, then horizontally, staying near the element.
    for k in 1..=12i64 {
        let dy = k * (chip_h as i64 + 2);
        candidates.push((left, clamp_y(by as i64 - chip_h as i64 / 2 + dy)));
        candidates.push((left, clamp_y(by as i64 - chip_h as i64 / 2 - dy)));
    }
    for k in 1..=12i64 {
        let dx = k * (chip_w as i64 + 2);
        candidates.push((clamp_x(bx as i64 + dx), on));
        candidates.push((clamp_x(bx as i64 - dx), on));
    }

    for (x, y) in candidates {
        let rect = (x, y, chip_w, chip_h);
        if !placed.iter().any(|p| rects_overlap(*p, rect, 2)) {
            return rect;
        }
    }
    (left, on, chip_w, chip_h)
}

/// Axis-aligned overlap test with a `gap` margin so chips keep a little air
/// between them rather than sitting flush.
fn rects_overlap(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32), gap: u32) -> bool {
    let ax1 = a.0.saturating_sub(gap);
    let ay1 = a.1.saturating_sub(gap);
    let ax2 = a.0 + a.2 + gap;
    let ay2 = a.1 + a.3 + gap;
    ax1 < b.0 + b.2 && b.0 < ax2 && ay1 < b.1 + b.3 && b.1 < ay2
}

#[allow(clippy::too_many_arguments)]
fn draw_hint_chip(
    c: &mut Canvas<'_>,
    colors: &crate::config::Colors,
    scale: u32,
    cw: u32,
    chip_h: u32,
    el: &HintElement,
    x: u32,
    y: u32,
    chip_w: u32,
    typed: &[char],
    matching: bool,
) {
    let typed_full = matching && !typed.is_empty() && el.label.chars().eq(typed.iter().copied());
    let text_color = if typed_full {
        colors.hint_text_typed
    } else if matching {
        colors.hint_text
    } else {
        colors.hint_dim
    };
    let bg = if matching {
        colors.hint_chip_bg
    } else {
        [
            colors.hint_chip_bg[0],
            colors.hint_chip_bg[1],
            colors.hint_chip_bg[2],
            colors.hint_chip_bg[3] / 3,
        ]
    };
    c.fill_rect(x, y, chip_w, chip_h, bg);
    // Border only on active chips — keeps dimmed neighbours visually quiet.
    if matching && chip_w > 2 && chip_h > 2 {
        c.fill_rect(x, y, chip_w, 1, colors.border);
        c.fill_rect(x, y + chip_h - 1, chip_w, 1, colors.border);
        c.fill_rect(x, y, 1, chip_h, colors.border);
        c.fill_rect(x + chip_w - 1, y, 1, chip_h, colors.border);
    }
    for (idx, ch) in el.label.chars().enumerate() {
        let color = if matching && idx < typed.len() {
            colors.hint_text_typed
        } else {
            text_color
        };
        c.draw_glyph(x + 4 + idx as u32 * cw, y + 3, ch, color, scale);
    }
}

fn draw_crosshair(c: &mut Canvas<'_>, colors: &crate::config::Colors, cx: u32, cy: u32) {
    let arm = 8;
    c.fill_rect(
        cx.saturating_sub(arm),
        cy,
        arm * 2 + 1,
        1,
        colors.hint_text_typed,
    );
    c.fill_rect(
        cx,
        cy.saturating_sub(arm),
        1,
        arm * 2 + 1,
        colors.hint_text_typed,
    );
}

pub fn render_macro_bind_key(buf: &mut [u8], w: u32, h: u32) {
    let colors = &config().colors;
    let mut p = Panel::new(buf, w, h, 6);
    p.text(b"save macro", colors.text_first)
        .skip()
        .text(b"press a key to bind", colors.text_white)
        .text(b"enter to skip binding", colors.text_grey)
        .text(b"escape to cancel", colors.text_grey);
}

pub fn render_macro_name(buf: &mut [u8], w: u32, h: u32, name: &[char], bind_key: Option<char>) {
    let colors = &config().colors;
    let mut p = Panel::new(buf, w, h, 7);
    p.text(b"name this macro", colors.text_first);
    match bind_key {
        Some(k) => p.text_with_char(b"bound to ", k, colors.text_grey),
        None => p.skip(),
    };
    p.input_line(name, colors.text_white)
        .skip()
        .text(b"enter to save", colors.text_grey)
        .text(b"escape to cancel", colors.text_grey);
}

pub fn render_macro_replay_wait(buf: &mut [u8], w: u32, h: u32) {
    let colors = &config().colors;
    let mut p = Panel::new(buf, w, h, 4);
    p.text(b"press macro key", colors.text_first)
        .skip()
        .text(b"escape to cancel", colors.text_grey);
}

pub fn render_macro_search(
    buf: &mut [u8],
    w: u32,
    h: u32,
    query: &[char],
    results: &[(Option<char>, &str)],
    selected: usize,
) {
    let colors = &config().colors;
    let max_visible = 10usize;
    let visible = results.len().min(max_visible);
    let mut p = Panel::new(buf, w, h, visible as u32 + 5);
    p.input_line(query, colors.text_white).skip();
    if results.is_empty() {
        p.text(b"no results", colors.text_grey);
    } else {
        for (i, (bind_key, name)) in results[..visible].iter().enumerate() {
            p.search_entry(*bind_key, name, i == selected);
        }
    }
    p.skip()
        .text(b"tab:next enter:select esc:back", colors.text_grey);
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
        let cfg = config();
        let mut c = Canvas { buf, w };
        c.clear();
        let scale = cfg.panel_font_size();
        let lh = line_height(scale);
        let panel_h = (rows * lh + 32).min(h.saturating_sub(4));
        let min_panel_chars = 30;
        let panel_padding = 16 * scale;
        let panel_min_w = min_panel_chars * char_width(scale) + panel_padding * 2;
        let panel_w = (w * 30 / 100).max(panel_min_w).min(w);
        let panel_x = (w.saturating_sub(panel_w)) / 2;
        let panel_y = (h.saturating_sub(panel_h)) / 2;
        c.fill_rect(panel_x, panel_y, panel_w, panel_h, cfg.colors.panel_bg);
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
        let colors = &config().colors;
        let cw = char_width(self.scale);
        let lh = line_height(self.scale);
        if selected {
            self.c.fill_rect(
                self.px + 4,
                self.ty.saturating_sub(2),
                self.pw - 8,
                lh,
                colors.selected_bg,
            );
        }
        let text_color = if selected {
            colors.text_highlight
        } else {
            colors.text_white
        };
        match bind_key {
            Some(k) => {
                self.c
                    .draw_text(self.tx, self.ty, b"[", colors.text_grey, self.scale);
                self.c
                    .draw_glyph(self.tx + cw, self.ty, k, colors.text_grey, self.scale);
                self.c.draw_text(
                    self.tx + 2 * cw,
                    self.ty,
                    b"] ",
                    colors.text_grey,
                    self.scale,
                );
            }
            None => self
                .c
                .draw_text(self.tx, self.ty, b"[ ] ", colors.text_grey, self.scale),
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
    let cfg = config();
    let nsub_cols = cfg.sub_cols();
    let nsub_rows = cfg.sub_rows();
    let sub_hints = cfg.sub_hints();
    let ncols = cfg.dynamic_cols(c.w);
    let nrows = cfg.dynamic_rows(h);

    // Early return if main_col/main_row are outside the rendered grid
    if main_col >= ncols || main_row >= nrows {
        return;
    }

    let cell_w = c.w / ncols;
    let cell_h = h / nrows;
    let cell_x = main_col * cell_w;
    let cell_y = main_row * cell_h;
    let colors = &cfg.colors;

    c.fill_rect(cell_x, cell_y, cell_w, cell_h, colors.sub_bg);

    let border = if dragging {
        colors.border_dragging
    } else {
        colors.border
    };
    c.fill_rect(cell_x, cell_y, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y + cell_h - 1, cell_w, 1, border);
    c.fill_rect(cell_x, cell_y, 1, cell_h, border);
    c.fill_rect(cell_x + cell_w - 1, cell_y, 1, cell_h, border);

    let sub_hint_scale = cfg.sub_hint_font_size();
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
                (colors.cell_highlight, colors.text_highlight)
            } else {
                (colors.sub_cell_normal, colors.text_first)
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

    // Inner separators between sub-cells (at the sub-span boundaries, skipping
    // the outer frame drawn above in `border`). Transparent by default.
    let separator = colors.separator_subgrid.unwrap_or([0, 0, 0, 0]);
    if separator[3] != 0 {
        for sub_col in 1..nsub_cols {
            let (x, _) = col_spans[sub_col as usize];
            c.fill_rect(x, cell_y, 1, cell_h, separator);
        }
        for sub_row in 1..nsub_rows {
            let (y, _) = row_spans[sub_row as usize];
            c.fill_rect(cell_x, y, cell_w, 1, separator);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coincident_elements_get_non_overlapping_chips() {
        // Three targets sharing the same tiny box (e.g. stacked controls). Their
        // chips must end up at distinct, non-overlapping positions.
        let el = |label: &str| HintElement {
            cx: 110,
            cy: 110,
            bbox: (100, 100, 20, 20),
            label: label.to_string(),
        };
        let elements = vec![el("a"), el("s"), el("d")];
        let chip_h = 14;
        let placements = place_chips(&elements, 1920, 1080, chip_h, 16);
        let rects: Vec<(u32, u32, u32, u32)> = placements
            .iter()
            .map(|&(x, y, cw)| (x, y, cw, chip_h))
            .collect();
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                assert!(
                    !rects_overlap(rects[i], rects[j], 0),
                    "chips {i} {:?} and {j} {:?} overlap",
                    rects[i],
                    rects[j]
                );
            }
        }
    }
}
