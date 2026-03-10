use crate::{
    app::replay_macro,
    backend::{Backend, KeyEvent},
    input::InputState,
    macro_store::MacroStore,
    mode::{Mode, ModeTransition},
    render::render_macro_replay_wait,
};

pub(super) fn handle_key<B: Backend>(
    width: u32,
    height: u32,
    key: &KeyEvent,
    backend: &mut B,
    macro_store: &MacroStore,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Escape => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::Char(ch) => {
            if let Some(entry) = macro_store.find_by_key(*ch).cloned() {
                replay_macro(&entry.actions, width, height, backend)?;
                Ok(ModeTransition::Exit)
            } else {
                Ok(ModeTransition::Enter(Mode::Normal {
                    input_state: InputState::First,
                    target: None,
                    drag_origin: None,
                }))
            }
        }
        _ => Ok(ModeTransition::Stay),
    }
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    render_macro_replay_wait(pixels, width, height);
    backend.present(pixels, width, height)
}
