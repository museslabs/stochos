use anyhow::Result;

use crate::backend::Backend;
use crate::input::{keys_to_pos, InputState};
use crate::macro_store::{MacroAction, MacroStore};
use crate::mode::{Mode, ModeTransition};
use crate::render::{render_grid, render_rec_indicator};

pub fn run<B: Backend>(backend: &mut B) -> Result<()> {
    let (w, h) = backend.screen_size();
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let mut macro_store = MacroStore::load();
    let mut transition_stack: Vec<Mode> = Vec::new();
    let mut mode = Mode::Normal {
        input_state: InputState::First,
        target: None,
        drag_origin: None,
    };

    mode.draw(backend, &mut pixels, w, h, &macro_store)?;

    while let Some(key) = backend.next_key()? {
        match mode.handle_key(w, h, backend, &key, &mut macro_store)? {
            ModeTransition::Stay => continue,
            ModeTransition::Enter(m) => {
                let prev = std::mem::replace(&mut mode, m);
                transition_stack.push(prev);
                mode.draw(backend, &mut pixels, w, h, &macro_store)?;
            }
            ModeTransition::Back => {
                if let Some(prev) = transition_stack.pop() {
                    mode = prev;
                    mode.draw(backend, &mut pixels, w, h, &macro_store)?;
                }
            }
            ModeTransition::Exit => {
                backend.exit()?;
                break;
            }
        };
    }

    Ok(())
}

pub fn draw_grid(
    pixels: &mut [u8],
    w: u32,
    h: u32,
    state: &InputState,
    dragging: bool,
    recording: bool,
    backend: &mut dyn Backend,
) -> Result<()> {
    render_grid(pixels, w, h, state, dragging);
    if recording {
        render_rec_indicator(pixels, w);
    }
    backend.present(pixels, w, h)
}

pub fn replay_macro(
    actions: &[MacroAction],
    w: u32,
    h: u32,
    backend: &mut dyn Backend,
) -> Result<()> {
    for action in actions {
        match action {
            MacroAction::Move(keys) => {
                if let Some((x, y)) = keys_to_pos(keys, w, h) {
                    backend.move_mouse(x, y)?;
                }
            }
            MacroAction::Click(keys) => {
                if let Some((x, y)) = keys_to_pos(keys, w, h) {
                    backend.click(x, y)?;
                }
            }
            MacroAction::DoubleClick(keys) => {
                if let Some((x, y)) = keys_to_pos(keys, w, h) {
                    backend.double_click(x, y)?;
                }
            }
            MacroAction::Drag(start_keys, end_keys) => {
                if let (Some((x1, y1)), Some((x2, y2))) =
                    (keys_to_pos(start_keys, w, h), keys_to_pos(end_keys, w, h))
                {
                    backend.drag_select(x1, y1, x2, y2)?;
                }
            }
        }
    }
    Ok(())
}
