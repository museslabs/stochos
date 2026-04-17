use crate::backend::Backend;
use crate::input::InputState;
use crate::macro_store::MacroStore;
use crate::mode::{Mode, ModeTransition};

#[derive(Clone, Copy)]
pub enum InitialMode {
    Normal,
    Bisect,
}

pub fn run<B: Backend>(backend: &mut B, initial: InitialMode) -> anyhow::Result<()> {
    let (w, h) = backend.screen_size();
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let mut macro_store = MacroStore::load();
    let mut transition_stack: Vec<Mode> = Vec::new();
    let mut mode = match initial {
        InitialMode::Normal => Mode::Normal {
            input_state: InputState::First,
            target: None,
            drag_origin: None,
        },
        InitialMode::Bisect => Mode::Bisect {
            region: (0, 0, w, h),
        },
    };

    backend.move_mouse(w / 2, h / 2)?;

    mode.draw(backend, &mut pixels, w, h, &macro_store)?;

    while let Some(key) = backend.next_key()? {
        match mode.handle_key(w, h, backend, &key, &mut macro_store)? {
            ModeTransition::Stay => continue,
            ModeTransition::Redraw => {
                mode.draw(backend, &mut pixels, w, h, &macro_store)?;
            }
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
