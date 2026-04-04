use super::Mode;
use anyhow::Ok;

use crate::{
    backend::{Backend, KeyEvent},
    input::{dynamic_cols, dynamic_rows, hints, sub_cols, sub_hints, sub_rows, InputState},
    macro_store::MacroAction,
    mode::{draw_grid, ModeTransition},
};

pub(super) fn handle_key<B: Backend>(
    width: u32,
    height: u32,
    key: &KeyEvent,
    backend: &mut B,
    input_state: &InputState,
    target: Option<(u32, u32)>,
    drag_origin: Option<(u32, u32)>,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Close => Ok(ModeTransition::Exit),
        KeyEvent::Undo => Ok(ModeTransition::Back),
        KeyEvent::Click => {
            if let Some((x, y)) = target {
                if let Some((ox, oy)) = drag_origin {
                    backend.drag_select(ox, oy, x, y)?;
                } else {
                    backend.click(x, y)?;
                }
            }
            Ok(ModeTransition::Exit)
        }
        KeyEvent::DoubleClick => {
            if let Some((x, y)) = target {
                if let Some((ox, oy)) = drag_origin {
                    backend.drag_select(ox, oy, x, y)?;
                } else {
                    backend.double_click(x, y)?;
                }
            }
            Ok(ModeTransition::Exit)
        }
        KeyEvent::RightClick if drag_origin.is_none() => {
            if let Some((x, y)) = target {
                backend.right_click(x, y)?;
            }
            Ok(ModeTransition::Exit)
        }
        KeyEvent::Char('/')
            if matches!(
                input_state,
                InputState::Ready { .. } | InputState::SubFirst { .. }
            ) =>
        {
            Ok(ModeTransition::Enter(Mode::Normal {
                input_state: InputState::First,
                target: None,
                drag_origin: target,
            }))
        }
        KeyEvent::Char('@')
            if matches!(input_state, InputState::First) && drag_origin.is_none() =>
        {
            Ok(ModeTransition::Enter(Mode::MacroReplayWait))
        }
        KeyEvent::MacroRecord
            if matches!(input_state, InputState::First) && drag_origin.is_none() =>
        {
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: Vec::new(),
                drag_start_keys: String::new(),
            }))
        }
        KeyEvent::Char(ch)
            if hints().contains(ch)
                || (matches!(input_state, InputState::SubFirst { .. })
                    && sub_hints().contains(ch)) =>
        {
            match input_state {
                InputState::First => Ok(ModeTransition::Enter(Mode::Normal {
                    input_state: InputState::Second(*ch),
                    target,
                    drag_origin,
                })),
                InputState::Second(first) => {
                    let col = hints().iter().position(|c| c == first).unwrap_or(0) as u32;
                    let row = hints().iter().position(|c| c == ch).unwrap_or(0) as u32;
                    let ncols = dynamic_cols(width);
                    let nrows = dynamic_rows(height);

                    // Reject if outside the currently rendered grid
                    if col >= ncols || row >= nrows {
                        return Ok(ModeTransition::Stay);
                    }

                    let cell_w = width / ncols;
                    let cell_h = height / nrows;
                    let cx = col * cell_w + cell_w / 2;
                    let cy = row * cell_h + cell_h / 2;

                    backend.move_mouse(cx, cy)?;

                    Ok(ModeTransition::Enter(Mode::Normal {
                        input_state: InputState::SubFirst { col, row },
                        target: Some((cx, cy)),
                        drag_origin,
                    }))
                }
                InputState::SubFirst { col, row } => {
                    if let Some(idx) = sub_hints().iter().position(|c| c == ch) {
                        let sub_col = idx as u32 % sub_cols();
                        let sub_row = idx as u32 / sub_cols();
                        let ncols = dynamic_cols(width);
                        let nrows = dynamic_rows(height);
                        let cell_w = width / ncols;
                        let cell_h = height / nrows;
                        let sub_cell_w = cell_w / sub_cols();
                        let sub_cell_h = cell_h / sub_rows();
                        let cx = col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2;
                        let cy = row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2;

                        backend.move_mouse(cx, cy)?;

                        return Ok(ModeTransition::Enter(Mode::Normal {
                            input_state: InputState::Ready {
                                col: *col,
                                row: *row,
                                sub_col,
                                sub_row,
                            },
                            target: Some((cx, cy)),
                            drag_origin,
                        }));
                    }
                    Ok(ModeTransition::Stay)
                }
                InputState::Ready { .. } => Ok(ModeTransition::Stay),
            }
        }
        KeyEvent::MacroMenu
            if matches!(
                input_state,
                InputState::SubFirst { .. } | InputState::Ready { .. }
            ) =>
        {
            Ok(ModeTransition::Enter(Mode::MacroBindKey {
                actions: vec![MacroAction::Click(input_state.keys())],
            }))
        }
        KeyEvent::MacroMenu
            if matches!(input_state, InputState::First) && drag_origin.is_none() =>
        {
            Ok(ModeTransition::Enter(Mode::MacroSearch {
                query: Vec::new(),
                selected: 0,
            }))
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
        _ => Ok(ModeTransition::Stay),
    }
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    input_state: &InputState,
    dragging: bool,
) -> anyhow::Result<()> {
    draw_grid(pixels, width, height, input_state, dragging, false, backend)
}
