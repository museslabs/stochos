use crate::{
    backend::{Backend, KeyEvent},
    input::InputState,
    macro_store::{MacroAction, MacroEntry, MacroStore},
    mode::{Mode, ModeTransition},
    render::render_macro_name,
};

pub(super) fn handle_key(
    key: &KeyEvent,
    bind_key: Option<char>,
    name: &[char],
    actions: &[MacroAction],
    macro_store: &mut MacroStore,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Close => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::DoubleClick => {
            let name_str = if name.is_empty() {
                format!("macro {}", macro_store.macros.len() + 1)
            } else {
                name.iter().collect()
            };
            macro_store.add(MacroEntry {
                name: name_str,
                actions: actions.to_vec(),
                bind_key,
            });
            macro_store.save()?;
            Ok(ModeTransition::Enter(Mode::Normal {
                input_state: InputState::First,
                target: None,
                drag_origin: None,
            }))
        }
        KeyEvent::Undo => {
            let mut name = name.to_vec();
            name.pop();
            Ok(ModeTransition::Enter(Mode::MacroName {
                bind_key,
                name,
                actions: actions.to_vec(),
            }))
        }
        KeyEvent::Char(ch) => {
            let mut name = name.to_vec();
            name.push(*ch);
            Ok(ModeTransition::Enter(Mode::MacroName {
                bind_key,
                name,
                actions: actions.to_vec(),
            }))
        }
        KeyEvent::Click => {
            let mut name = name.to_vec();
            name.push(' ');
            Ok(ModeTransition::Enter(Mode::MacroName {
                bind_key,
                name,
                actions: actions.to_vec(),
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
    name: &[char],
    bind_key: Option<char>,
) -> anyhow::Result<()> {
    render_macro_name(pixels, width, height, name, bind_key);
    backend.present(pixels, width, height)
}
