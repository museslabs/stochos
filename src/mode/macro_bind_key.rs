use crate::{
    backend::{Backend, KeyEvent},
    input::InputState,
    macro_store::MacroAction,
    mode::{Mode, ModeTransition},
    render::render_macro_bind_key,
};

pub(super) fn handle_key(
    key: &KeyEvent,
    actions: &[MacroAction],
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Escape => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::Enter => Ok(ModeTransition::Enter(Mode::MacroName {
            bind_key: None,
            name: Vec::new(),
            actions: actions.to_vec(),
        })),
        KeyEvent::Char(ch) => Ok(ModeTransition::Enter(Mode::MacroName {
            bind_key: Some(*ch),
            name: Vec::new(),
            actions: actions.to_vec(),
        })),
        _ => Ok(ModeTransition::Stay),
    }
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    render_macro_bind_key(pixels, width, height);
    backend.present(pixels, width, height)
}
