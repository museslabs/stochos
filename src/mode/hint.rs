use std::rc::Rc;

use crate::{
    backend::{Backend, KeyEvent},
    config::config,
    hint::{build_hint_mode, label_starts_with, HintElement},
    mode::{Mode, ModeTransition},
    render::render_hints,
};

/// Build and enter hint mode, degrading gracefully when screen capture is
/// unavailable (e.g. a compositor without `zwlr_screencopy_v1`). A failed
/// capture must not tear down the whole app mid-session, so we report it and
/// stay in the current mode rather than propagating the error up the run loop.
pub(super) fn enter<B: Backend>(backend: &mut B, width: u32, height: u32) -> ModeTransition {
    match build_hint_mode(backend, width, height) {
        Ok(elements) => ModeTransition::Enter(Mode::Hint {
            elements: elements.into(),
            typed: Vec::new(),
            target: None,
        }),
        Err(e) => {
            eprintln!("hint mode unavailable: {e:#}");
            ModeTransition::Stay
        }
    }
}

pub(super) fn handle_key<B: Backend>(
    key: &KeyEvent,
    backend: &mut B,
    elements: &Rc<[HintElement]>,
    typed: &[char],
    target: Option<(u32, u32)>,
) -> anyhow::Result<ModeTransition> {
    match key {
        KeyEvent::Close => Ok(ModeTransition::Exit),
        KeyEvent::Undo => {
            if typed.is_empty() {
                Ok(ModeTransition::Back)
            } else {
                let mut next_typed = typed.to_vec();
                next_typed.pop();
                Ok(ModeTransition::Replace(Mode::Hint {
                    elements: Rc::clone(elements),
                    typed: next_typed,
                    target: None,
                }))
            }
        }
        KeyEvent::Click => action(backend, target, Backend::click),
        KeyEvent::DoubleClick => action(backend, target, Backend::double_click),
        KeyEvent::TripleClick => action(backend, target, Backend::triple_click),
        KeyEvent::RightClick => action(backend, target, Backend::right_click),
        KeyEvent::MiddleClick => action(backend, target, Backend::middle_click),
        KeyEvent::Char('/') => {
            if let Some((x, y)) = target {
                Ok(ModeTransition::Enter(Mode::Normal {
                    input_state: crate::input::InputState::First,
                    target: None,
                    drag_origin: Some((x, y)),
                }))
            } else {
                Ok(ModeTransition::Stay)
            }
        }
        KeyEvent::Char(ch) if config().hint_alphabet().contains(ch) => {
            let mut next_typed = typed.to_vec();
            next_typed.push(*ch);
            let matches: Vec<_> = elements
                .iter()
                .filter(|element| label_starts_with(&element.label, &next_typed))
                .collect();
            if matches.is_empty() {
                return Ok(ModeTransition::Stay);
            }

            if let Some(element) = matches
                .iter()
                .copied()
                .find(|element| element.label.chars().eq(next_typed.iter().copied()))
            {
                backend.move_mouse(element.cx, element.cy)?;
                if config().hint.auto_click {
                    backend.click(element.cx, element.cy)?;
                    return Ok(ModeTransition::Exit);
                }
                return Ok(ModeTransition::Replace(Mode::Hint {
                    elements: Rc::clone(elements),
                    typed: next_typed,
                    target: Some((element.cx, element.cy)),
                }));
            }

            Ok(ModeTransition::Replace(Mode::Hint {
                elements: Rc::clone(elements),
                typed: next_typed,
                target: None,
            }))
        }
        KeyEvent::FreeMode => {
            let (x, y) = if let Some((x, y)) = target {
                (x, y)
            } else {
                backend.mouse_pos()?
            };

            Ok(ModeTransition::Enter(Mode::Free {
                x,
                y,
                speed: config().free.base_speed.max(1),
            }))
        }
        _ => Ok(ModeTransition::Stay),
    }
}

fn action<B: Backend>(
    backend: &mut B,
    target: Option<(u32, u32)>,
    f: fn(&mut B, u32, u32) -> anyhow::Result<()>,
) -> anyhow::Result<ModeTransition> {
    if let Some((x, y)) = target {
        f(backend, x, y)?;
        Ok(ModeTransition::Exit)
    } else {
        Ok(ModeTransition::Stay)
    }
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    elements: &[HintElement],
    typed: &[char],
) -> anyhow::Result<()> {
    render_hints(pixels, width, height, elements, typed);
    backend.present(pixels, width, height)
}
