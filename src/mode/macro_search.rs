use crate::{
    app::replay_macro,
    backend::{Backend, KeyEvent},
    input::InputState,
    macro_store::MacroStore,
    mode::{Mode, ModeTransition},
    render::render_macro_search,
};

pub(super) fn handle_key<B: Backend>(
    width: u32,
    height: u32,
    key: &KeyEvent,
    backend: &mut B,
    query: &[char],
    selected: usize,
    macro_store: &MacroStore,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Escape => Ok(ModeTransition::Enter(Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        })),
        KeyEvent::Enter => {
            let results = macro_store.fuzzy_search(query);
            if let Some(entry) = results.get(selected).cloned() {
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
        KeyEvent::Tab => {
            let results = macro_store.fuzzy_search(query);
            let new_selected = if results.is_empty() {
                0
            } else {
                (selected + 1) % results.len()
            };
            Ok(ModeTransition::Enter(Mode::MacroSearch {
                query: query.to_vec(),
                selected: new_selected,
            }))
        }
        KeyEvent::Backspace => {
            let mut query = query.to_vec();
            query.pop();
            Ok(ModeTransition::Enter(Mode::MacroSearch {
                query,
                selected: 0,
            }))
        }
        KeyEvent::Char(ch) => {
            let mut query = query.to_vec();
            query.push(*ch);
            Ok(ModeTransition::Enter(Mode::MacroSearch {
                query,
                selected: 0,
            }))
        }
        KeyEvent::Space => {
            let mut query = query.to_vec();
            query.push(' ');
            Ok(ModeTransition::Enter(Mode::MacroSearch {
                query,
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
    query: &[char],
    selected: usize,
    macro_store: &MacroStore,
) -> anyhow::Result<()> {
    let results = macro_store.fuzzy_search(query);
    let items: Vec<(Option<char>, &str)> = results
        .iter()
        .map(|m| (m.bind_key, m.name.as_str()))
        .collect();
    render_macro_search(pixels, width, height, query, &items, selected);
    backend.present(pixels, width, height)
}
