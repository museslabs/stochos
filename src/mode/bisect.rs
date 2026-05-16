use crate::{
    backend::{Backend, KeyEvent},
    config::config,
    input::InputState,
    mode::{Mode, ModeTransition},
    render::render_bisect,
};

fn subcell_size(region: (u32, u32, u32, u32), rows: u32, cols: u32) -> (u32, u32) {
    let (_, _, rw, rh) = region;
    (rw / cols.max(1), rh / rows.max(1))
}

pub(super) fn handle_key<B: Backend>(
    key: &KeyEvent,
    backend: &mut B,
    region: (u32, u32, u32, u32),
) -> anyhow::Result<ModeTransition> {
    let (rx, ry, rw, rh) = region;
    let cx = rx + rw / 2;
    let cy = ry + rh / 2;
    match key {
        KeyEvent::Close => Ok(ModeTransition::Exit),
        KeyEvent::Undo => Ok(ModeTransition::Back),
        KeyEvent::Click => {
            backend.click(cx, cy)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::DoubleClick => {
            backend.double_click(cx, cy)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::RightClick => {
            backend.right_click(cx, cy)?;
            Ok(ModeTransition::Exit)
        }
        KeyEvent::ScrollUp => {
            backend.scroll_up()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollDown => {
            backend.scroll_down()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollLeft => {
            backend.scroll_left()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::ScrollRight => {
            backend.scroll_right()?;
            Ok(ModeTransition::Redraw)
        }
        KeyEvent::Char(ch) => {
            let cfg = &config().bisect;
            let rows = cfg.rows.max(1);
            let cols = cfg.cols.max(1);
            let (sub_w_approx, sub_h_approx) = subcell_size(region, rows, cols);
            if sub_w_approx < cfg.min_cell_size || sub_h_approx < cfg.min_cell_size {
                return Ok(ModeTransition::Stay);
            }
            let Some(idx) = cfg.hints.iter().position(|c| c == ch) else {
                return Ok(ModeTransition::Stay);
            };
            let idx = idx as u32;
            if idx >= rows * cols {
                return Ok(ModeTransition::Stay);
            }
            let col = idx % cols;
            let row = idx / cols;
            let sub_x = rx + col * rw / cols;
            let sub_x_end = rx + (col + 1) * rw / cols;
            let sub_y = ry + row * rh / rows;
            let sub_y_end = ry + (row + 1) * rh / rows;
            let sub_w = sub_x_end.saturating_sub(sub_x);
            let sub_h = sub_y_end.saturating_sub(sub_y);
            backend.move_mouse(sub_x + sub_w / 2, sub_y + sub_h / 2)?;
            Ok(ModeTransition::Enter(Mode::Bisect {
                region: (sub_x, sub_y, sub_w, sub_h),
            }))
        }
        KeyEvent::Normal => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::FreeMode => Ok(ModeTransition::Enter(Mode::Free {
            x: cx,
            y: cy,
            speed: config().free.base_speed.max(1),
        })),
        _ => Ok(ModeTransition::Stay),
    }
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    region: (u32, u32, u32, u32),
) -> anyhow::Result<()> {
    render_bisect(pixels, width, height, region);
    backend.present(pixels, width, height)
}
