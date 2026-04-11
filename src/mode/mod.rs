mod macro_bind_key;
mod macro_name;
mod macro_replay;
mod macro_search;
mod normal;
mod recording;

use crate::{
    backend::{Backend, KeyEvent},
    config::config,
    input::{keys_to_pos, InputState},
    macro_store::{MacroAction, MacroStore},
    render::{render_grid, render_rec_indicator},
};

pub(super) fn move_to_column_center<B: Backend>(
    backend: &mut B,
    col: u32,
    width: u32,
    height: u32,
) -> anyhow::Result<()> {
    let cfg = config();
    let ncols = cfg.dynamic_cols(width);
    if col < ncols {
        let cell_w = width / ncols;
        let cx = col * cell_w + cell_w / 2;
        let cy = height / 2;
        backend.move_mouse(cx, cy)?;
    }
    Ok(())
}

pub enum ModeTransition {
    Stay,
    Redraw,
    Enter(Mode),
    Back,
    Exit,
}

pub enum Mode {
    Normal {
        input_state: InputState,
        target: Option<(u32, u32)>,
        drag_origin: Option<(u32, u32)>,
    },
    MacroRecording {
        input_state: InputState,
        target: Option<(u32, u32)>,
        drag_origin: Option<(u32, u32)>,
        recorded_actions: Vec<MacroAction>,
        drag_start_keys: String,
    },
    MacroBindKey {
        actions: Vec<MacroAction>,
    },
    MacroName {
        bind_key: Option<char>,
        name: Vec<char>,
        actions: Vec<MacroAction>,
    },
    MacroReplayWait,
    MacroSearch {
        query: Vec<char>,
        selected: usize,
    },
}

impl Mode {
    pub fn handle_key<B: Backend>(
        &self,
        width: u32,
        height: u32,
        backend: &mut B,
        key: &KeyEvent,
        macro_store: &mut MacroStore,
    ) -> anyhow::Result<ModeTransition> {
        match self {
            Mode::Normal {
                input_state,
                target,
                drag_origin,
            } => normal::handle_key(
                width,
                height,
                key,
                backend,
                input_state,
                *target,
                *drag_origin,
            ),
            Mode::MacroRecording {
                input_state,
                target,
                drag_origin,
                recorded_actions,
                drag_start_keys,
            } => recording::handle_key(
                width,
                height,
                key,
                backend,
                input_state,
                *target,
                *drag_origin,
                recorded_actions,
                drag_start_keys,
            ),
            Mode::MacroBindKey { actions } => macro_bind_key::handle_key(key, actions),
            Mode::MacroName {
                bind_key,
                name,
                actions,
            } => macro_name::handle_key(key, *bind_key, name, actions, macro_store),
            Mode::MacroReplayWait => {
                macro_replay::handle_key(width, height, key, backend, macro_store)
            }
            Mode::MacroSearch { query, selected } => {
                macro_search::handle_key(width, height, key, backend, query, *selected, macro_store)
            }
        }
    }

    pub fn draw<B: Backend>(
        &self,
        backend: &mut B,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        macro_store: &MacroStore,
    ) -> anyhow::Result<()> {
        match self {
            Mode::Normal {
                input_state,
                drag_origin,
                ..
            } => normal::draw(
                backend,
                pixels,
                width,
                height,
                input_state,
                drag_origin.is_some(),
            ),
            Mode::MacroRecording {
                input_state,
                drag_origin,
                ..
            } => recording::draw(
                backend,
                pixels,
                width,
                height,
                input_state,
                drag_origin.is_some(),
            ),
            Mode::MacroBindKey { .. } => macro_bind_key::draw(backend, pixels, width, height),
            Mode::MacroName { bind_key, name, .. } => {
                macro_name::draw(backend, pixels, width, height, name, *bind_key)
            }
            Mode::MacroReplayWait => macro_replay::draw(backend, pixels, width, height),
            Mode::MacroSearch { query, selected } => macro_search::draw(
                backend,
                pixels,
                width,
                height,
                query,
                *selected,
                macro_store,
            ),
        }
    }
}

pub(super) fn draw_grid(
    pixels: &mut [u8],
    w: u32,
    h: u32,
    state: &InputState,
    dragging: bool,
    recording: bool,
    backend: &mut dyn Backend,
) -> anyhow::Result<()> {
    render_grid(pixels, w, h, state, dragging);
    if recording {
        render_rec_indicator(pixels, w);
    }
    backend.present(pixels, w, h)
}

pub(super) fn replay_macro(
    actions: &[MacroAction],
    w: u32,
    h: u32,
    backend: &mut dyn Backend,
) -> anyhow::Result<()> {
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
            MacroAction::RightClick(keys) => {
                if let Some((x, y)) = keys_to_pos(keys, w, h) {
                    backend.right_click(x, y)?;
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
