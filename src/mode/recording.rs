use std::time::Instant;

use crate::{
    backend::{Backend, KeyEvent},
    config::config,
    input::InputState,
    macro_store::{MacroAction, MacroActionKind},
    mode::{draw_grid, move_to_column_center, Mode, ModeTransition},
};

fn elapsed_ms(last: Option<Instant>, now: Instant) -> u64 {
    last.map(|t| now.duration_since(t).as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn handle_key<B: Backend>(
    width: u32,
    height: u32,
    key: &KeyEvent,
    backend: &mut B,
    input_state: &InputState,
    target: Option<(u32, u32)>,
    drag_origin: Option<(u32, u32)>,
    recorded_actions: &[MacroAction],
    drag_start_keys: &str,
    last_action_at: Option<Instant>,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Undo => Ok(ModeTransition::Back),
        KeyEvent::MacroRecord => {
            if recorded_actions.is_empty() {
                backend.move_mouse(width / 2, height / 2)?;
                Ok(ModeTransition::Enter(Mode::Normal {
                    input_state: InputState::First,
                    target: None,
                    drag_origin: None,
                }))
            } else {
                Ok(ModeTransition::Enter(Mode::MacroBindKey {
                    actions: recorded_actions.to_vec(),
                }))
            }
        }
        KeyEvent::Close => {
            backend.move_mouse(width / 2, height / 2)?;
            Ok(ModeTransition::Enter(Mode::Normal {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
            }))
        }
        KeyEvent::Char(ch)
            if config().hints().contains(ch)
                || (matches!(input_state, InputState::SubFirst { .. })
                    && config().sub_hints().contains(ch)) =>
        {
            let cfg = config();
            match input_state {
                InputState::First => {
                    let col = cfg.hints().iter().position(|c| c == ch).unwrap_or(0) as u32;
                    move_to_column_center(backend, col, width, height)?;

                    Ok(ModeTransition::Enter(Mode::MacroRecording {
                        input_state: InputState::Second(*ch),
                        target,
                        drag_origin,
                        recorded_actions: recorded_actions.to_vec(),
                        drag_start_keys: drag_start_keys.to_owned(),
                        last_action_at,
                    }))
                }
                InputState::Second(first) => {
                    let col = cfg.hints().iter().position(|c| c == first).unwrap_or(0) as u32;
                    let row = cfg.hints().iter().position(|c| c == ch).unwrap_or(0) as u32;
                    let ncols = cfg.dynamic_cols(width);
                    let nrows = cfg.dynamic_rows(height);

                    // Reject if outside the currently rendered grid
                    if col >= ncols || row >= nrows {
                        return Ok(ModeTransition::Stay);
                    }

                    let cell_w = width / ncols;
                    let cell_h = height / nrows;
                    let cx = col * cell_w + cell_w / 2;
                    let cy = row * cell_h + cell_h / 2;

                    backend.move_mouse(cx, cy)?;

                    Ok(ModeTransition::Enter(Mode::MacroRecording {
                        input_state: InputState::SubFirst { col, row },
                        target: Some((cx, cy)),
                        drag_origin,
                        recorded_actions: recorded_actions.to_vec(),
                        drag_start_keys: drag_start_keys.to_owned(),
                        last_action_at,
                    }))
                }
                InputState::SubFirst { col, row } => {
                    if let Some(idx) = cfg.sub_hints().iter().position(|c| c == ch) {
                        let sub_col = idx as u32 % cfg.sub_cols();
                        let sub_row = idx as u32 / cfg.sub_cols();
                        let ncols = cfg.dynamic_cols(width);
                        let nrows = cfg.dynamic_rows(height);
                        let cell_w = width / ncols;
                        let cell_h = height / nrows;
                        let sub_cell_w = cell_w / cfg.sub_cols();
                        let sub_cell_h = cell_h / cfg.sub_rows();
                        let cx = col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2;
                        let cy = row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2;

                        backend.move_mouse(cx, cy)?;

                        return Ok(ModeTransition::Enter(Mode::MacroRecording {
                            input_state: InputState::Ready {
                                col: *col,
                                row: *row,
                                sub_col,
                                sub_row,
                            },
                            target: Some((cx, cy)),
                            drag_origin,
                            recorded_actions: recorded_actions.to_vec(),
                            drag_start_keys: drag_start_keys.to_owned(),
                            last_action_at,
                        }));
                    }
                    Ok(ModeTransition::Stay)
                }
                InputState::Ready { .. } => Ok(ModeTransition::Stay),
            }
        }
        KeyEvent::Click | KeyEvent::DoubleClick | KeyEvent::RightClick
            if target.is_some() && drag_origin.is_none() =>
        {
            let (x, y) = target.unwrap();
            let current_keys = input_state.keys();
            let now = Instant::now();
            let wait_ms = elapsed_ms(last_action_at, now);
            let mut new_actions = recorded_actions.to_vec();
            match key {
                KeyEvent::Click => {
                    backend.click(x, y)?;
                    new_actions.push(MacroAction::new(MacroActionKind::Click(current_keys), wait_ms));
                }
                KeyEvent::DoubleClick => {
                    backend.double_click(x, y)?;
                    new_actions.push(MacroAction::new(
                        MacroActionKind::DoubleClick(current_keys),
                        wait_ms,
                    ));
                }
                KeyEvent::RightClick => {
                    backend.right_click(x, y)?;
                    new_actions.push(MacroAction::new(
                        MacroActionKind::RightClick(current_keys),
                        wait_ms,
                    ));
                }
                _ => {}
            }
            backend.reopen()?;
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: new_actions,
                drag_start_keys: String::new(),
                last_action_at: Some(now),
            }))
        }
        KeyEvent::Click | KeyEvent::DoubleClick if target.is_some() => {
            let (x, y) = target.unwrap();
            let current_keys = input_state.keys();
            let now = Instant::now();
            let wait_ms = elapsed_ms(last_action_at, now);
            let mut new_actions = recorded_actions.to_vec();
            backend.drag_select(drag_origin.unwrap().0, drag_origin.unwrap().1, x, y)?;
            new_actions.push(MacroAction::new(
                MacroActionKind::Drag(drag_start_keys.to_owned(), current_keys),
                wait_ms,
            ));
            backend.reopen()?;
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: new_actions,
                drag_start_keys: String::new(),
                last_action_at: Some(now),
            }))
        }
        KeyEvent::MacroMenu
            if target.is_some()
                && drag_origin.is_none()
                && matches!(
                    input_state,
                    InputState::SubFirst { .. } | InputState::Ready { .. }
                ) =>
        {
            let now = Instant::now();
            let wait_ms = elapsed_ms(last_action_at, now);
            let mut new_actions = recorded_actions.to_vec();
            new_actions.push(MacroAction::new(
                MacroActionKind::Move(input_state.keys()),
                wait_ms,
            ));
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: new_actions,
                drag_start_keys: String::new(),
                last_action_at: Some(now),
            }))
        }
        KeyEvent::Char('/') if drag_origin.is_some() => {
            backend.move_mouse(width / 2, height / 2)?;
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: recorded_actions.to_vec(),
                drag_start_keys: String::new(),
                last_action_at: last_action_at.map(|_| Instant::now()),
            }))
        }
        KeyEvent::Char('/')
            if matches!(
                input_state,
                InputState::Ready { .. } | InputState::SubFirst { .. }
            ) =>
        {
            backend.move_mouse(width / 2, height / 2)?;
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target,
                drag_origin: target,
                recorded_actions: recorded_actions.to_vec(),
                drag_start_keys: input_state.keys(),
                last_action_at,
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
    draw_grid(pixels, width, height, input_state, dragging, true, backend)
}
