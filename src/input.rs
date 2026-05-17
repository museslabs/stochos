use crate::config::config;

#[derive(Clone, Copy)]
pub enum InputState {
    /// Waiting for the first main-grid character
    First,
    /// First main-grid character pressed; waiting for second
    Second(char),
    /// Main cell chosen; waiting for a single sub-grid character
    SubFirst { col: u32, row: u32 },
    /// Sub-cell chosen; mouse positioned, waiting for Space/Enter
    Ready {
        col: u32,
        row: u32,
        sub_col: u32,
        sub_row: u32,
    },
}

impl InputState {
    /// Returns the key string encoding the current navigation position.
    /// Returns an empty string for states that haven't reached a target yet.
    pub fn keys(&self) -> String {
        let cfg = config();
        match self {
            InputState::SubFirst { col, row } => {
                format!(
                    "{}{}",
                    cfg.hints()[*col as usize],
                    cfg.hints()[*row as usize]
                )
            }
            InputState::Ready {
                col,
                row,
                sub_col,
                sub_row,
            } => {
                format!(
                    "{}{}{}",
                    cfg.hints()[*col as usize],
                    cfg.hints()[*row as usize],
                    cfg.sub_hints()[(*sub_row * cfg.sub_cols() + *sub_col) as usize]
                )
            }
            _ => String::new(),
        }
    }
}

/// Converts a 2- or 3-character key string to a pixel position.
/// Returns None if the keys map to a position outside the current dynamic grid.
pub fn keys_to_pos(keys: &str, w: u32, h: u32) -> Option<(u32, u32)> {
    let cfg = config();
    let hints = cfg.hints();
    let mut chars = keys.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    let col = hints.iter().position(|&c| c == c0)? as u32;
    let row = hints.iter().position(|&c| c == c1)? as u32;
    let ncols = cfg.dynamic_cols(w);
    let nrows = cfg.dynamic_rows(h);

    // Reject hints that map outside the currently rendered grid
    if col >= ncols || row >= nrows {
        return None;
    }

    let cell_w = w / ncols;
    let cell_h = h / nrows;
    match chars.next() {
        None => Some((col * cell_w + cell_w / 2, row * cell_h + cell_h / 2)),
        Some(c2) => {
            let idx = cfg.sub_hints().iter().position(|&c| c == c2)? as u32;
            let sub_col = idx % cfg.sub_cols();
            let sub_row = idx / cfg.sub_cols();
            let sub_cell_w = cell_w / cfg.sub_cols();
            let sub_cell_h = cell_h / cfg.sub_rows();
            Some((
                col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2,
                row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2,
            ))
        }
    }
}
