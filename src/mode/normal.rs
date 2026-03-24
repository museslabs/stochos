use super::Mode;
use anyhow::Ok;

use crate::{
    backend::{Backend, KeyEvent},
    input::{InputState, COLS, HINTS, ROWS, SUB_COLS, SUB_HINTS, SUB_ROWS},
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
            if HINTS.contains(ch)
                || (matches!(input_state, InputState::SubFirst { .. })
                    && SUB_HINTS.contains(ch)) =>
        {
            match input_state {
                InputState::First => Ok(ModeTransition::Enter(Mode::Normal {
                    input_state: InputState::Second(*ch),
                    target,
                    drag_origin,
                })),
                InputState::Second(first) => {
                    let col = HINTS.iter().position(|c| c == first).unwrap_or(0) as u32;
                    let row = HINTS.iter().position(|c| c == ch).unwrap_or(0) as u32;
                    let cell_w = width / COLS;
                    let cell_h = height / ROWS;
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
                    if let Some(idx) = SUB_HINTS.iter().position(|c| c == ch) {
                        let sub_col = idx as u32 % SUB_COLS;
                        let sub_row = idx as u32 / SUB_COLS;
                        let cell_w = width / COLS;
                        let cell_h = height / ROWS;
                        let sub_cell_w = cell_w / SUB_COLS;
                        let sub_cell_h = cell_h / SUB_ROWS;
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
