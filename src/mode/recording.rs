use crate::{
    backend::{Backend, KeyEvent},
    input::{dynamic_cols, dynamic_rows, hints, sub_cols, sub_hints, sub_rows, InputState},
    macro_store::MacroAction,
    mode::{draw_grid, Mode, ModeTransition},
};

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
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Undo => Ok(ModeTransition::Back),
        KeyEvent::MacroRecord => {
            if recorded_actions.is_empty() {
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
        KeyEvent::Close => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::Char(ch)
            if hints().contains(ch)
                || (matches!(input_state, InputState::SubFirst { .. })
                    && sub_hints().contains(ch)) =>
        {
            match input_state {
                InputState::First => Ok(ModeTransition::Enter(Mode::MacroRecording {
                    input_state: InputState::Second(*ch),
                    target,
                    drag_origin,
                    recorded_actions: recorded_actions.to_vec(),
                    drag_start_keys: drag_start_keys.to_owned(),
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

                    Ok(ModeTransition::Enter(Mode::MacroRecording {
                        input_state: InputState::SubFirst { col, row },
                        target: Some((cx, cy)),
                        drag_origin,
                        recorded_actions: recorded_actions.to_vec(),
                        drag_start_keys: drag_start_keys.to_owned(),
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
            let mut new_actions = recorded_actions.to_vec();
            match key {
                KeyEvent::Click => {
                    backend.click(x, y)?;
                    new_actions.push(MacroAction::Click(current_keys));
                }
                KeyEvent::DoubleClick => {
                    backend.double_click(x, y)?;
                    new_actions.push(MacroAction::DoubleClick(current_keys));
                }
                KeyEvent::RightClick => {
                    backend.right_click(x, y)?;
                    new_actions.push(MacroAction::RightClick(current_keys));
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
            }))
        }
        KeyEvent::Click | KeyEvent::DoubleClick if target.is_some() => {
            let (x, y) = target.unwrap();
            let current_keys = input_state.keys();
            let mut new_actions = recorded_actions.to_vec();
            backend.drag_select(drag_origin.unwrap().0, drag_origin.unwrap().1, x, y)?;
            new_actions.push(MacroAction::Drag(drag_start_keys.to_owned(), current_keys));
            backend.reopen()?;
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: new_actions,
                drag_start_keys: String::new(),
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
            let mut new_actions = recorded_actions.to_vec();
            new_actions.push(MacroAction::Move(input_state.keys()));
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: new_actions,
                drag_start_keys: String::new(),
            }))
        }
        KeyEvent::Char('/') if drag_origin.is_some() => {
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
                recorded_actions: recorded_actions.to_vec(),
                drag_start_keys: String::new(),
            }))
        }
        KeyEvent::Char('/')
            if matches!(
                input_state,
                InputState::Ready { .. } | InputState::SubFirst { .. }
            ) =>
        {
            Ok(ModeTransition::Enter(Mode::MacroRecording {
                input_state: InputState::First,
                target,
                drag_origin: target,
                recorded_actions: recorded_actions.to_vec(),
                drag_start_keys: input_state.keys(),
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
