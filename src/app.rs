use anyhow::Result;

use crate::backend::{Backend, KeyEvent};
use crate::input::{InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS};
use crate::render::render_grid;

/// Main application loop — platform agnostic.
/// The backend handles all display and pointer operations;
/// this function owns the input state machine and rendering.
pub fn run(backend: &mut dyn Backend) -> Result<()> {
    let (w, h) = backend.screen_size();
    let mut state = InputState::First;
    let mut target: Option<(u32, u32)> = None;
    // When Some, we are in drag mode and this holds the drag start position.
    let mut drag_origin: Option<(u32, u32)> = None;
    let mut pixels = vec![0u8; (w * h * 4) as usize];

    render_grid(&mut pixels, w, h, &state, false);
    backend.present(&pixels, w, h)?;

    while let Some(key) = backend.next_key()? {
        match key {
            KeyEvent::Escape => {
                backend.exit()?;
                break;
            }
            KeyEvent::Space => {
                if let Some((x, y)) = target {
                    if let Some((ox, oy)) = drag_origin {
                        backend.drag_select(ox, oy, x, y)?;
                    } else {
                        backend.click(x, y)?;
                    }
                    break;
                }
            }
            KeyEvent::Enter => {
                if let Some((x, y)) = target {
                    if let Some((ox, oy)) = drag_origin {
                        backend.drag_select(ox, oy, x, y)?;
                    } else {
                        backend.double_click(x, y)?;
                    }
                    break;
                }
            }
            KeyEvent::Char(ch) => {
                if ch == b'/' && drag_origin.is_some() {
                    // Cancel drag mode and exit
                    backend.exit()?;
                    break;
                } else if ch == b'/'
                    && matches!(
                        state,
                        InputState::Ready { .. } | InputState::SubFirst { .. }
                    )
                {
                    // Enter drag mode: record origin, reset grid for end selection
                    drag_origin = target;
                    state = InputState::First;
                    render_grid(&mut pixels, w, h, &state, true);
                    backend.present(&pixels, w, h)?;
                } else if advance(&mut state, &mut target, ch, w, h, backend)? {
                    render_grid(&mut pixels, w, h, &state, drag_origin.is_some());
                    backend.present(&pixels, w, h)?;
                }
            }
        }
    }

    Ok(())
}

/// Advances the input state machine for a single character.
/// Calls `backend.move_mouse()` when the mouse position changes.
/// Returns true if the overlay needs to be redrawn.
fn advance(
    state: &mut InputState,
    target: &mut Option<(u32, u32)>,
    ch: u8,
    w: u32,
    h: u32,
    backend: &mut dyn Backend,
) -> Result<bool> {
    match *state {
        InputState::First => {
            if HINTS.contains(&ch) {
                *state = InputState::Second(ch);
                return Ok(true);
            }
        }
        InputState::Second(first) => {
            if HINTS.contains(&ch) {
                let col = HINTS.iter().position(|&c| c == first).unwrap_or(0) as u32;
                let row = HINTS.iter().position(|&c| c == ch).unwrap_or(0) as u32;
                let cell_w = w / COLS;
                let cell_h = h / ROWS;
                let cx = col * cell_w + cell_w / 2;
                let cy = row * cell_h + cell_h / 2;
                *target = Some((cx, cy));
                backend.move_mouse(cx, cy)?;
                *state = InputState::SubFirst { col, row };
                return Ok(true);
            }
        }
        InputState::SubFirst { col, row } => {
            if let Some(idx) = SUB_HINTS.iter().position(|&c| c == ch) {
                let sub_col = idx as u32 % SUB_COLS;
                let sub_row = idx as u32 / SUB_COLS;
                let cell_w = w / COLS;
                let cell_h = h / ROWS;
                let sub_cell_w = cell_w / SUB_COLS;
                let sub_cell_h = cell_h / SUB_ROWS;
                let cx = col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2;
                let cy = row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2;
                *target = Some((cx, cy));
                backend.move_mouse(cx, cy)?;
                *state = InputState::Ready {
                    col,
                    row,
                    sub_col,
                    sub_row,
                };
                return Ok(true);
            }
        }
        InputState::Ready { .. } => {}
    }
    Ok(false)
}
