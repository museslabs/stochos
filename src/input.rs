use crate::config::config;

pub fn hints() -> &'static [char] {
    &config().grid.hints
}

pub fn sub_hints() -> &'static [char] {
    &config().grid.sub_hints
}
pub fn sub_cols() -> u32 {
    config().grid.sub_cols
}
pub fn sub_rows() -> u32 {
    config().sub_rows()
}

/// Get dynamic columns based on screen width
pub fn dynamic_cols(screen_width: u32) -> u32 {
    config().dynamic_cols(screen_width)
}

/// Get dynamic rows based on screen height
pub fn dynamic_rows(screen_height: u32) -> u32 {
    config().dynamic_rows(screen_height)
}

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
        match self {
            InputState::SubFirst { col, row } => {
                format!("{}{}", hints()[*col as usize], hints()[*row as usize])
            }
            InputState::Ready {
                col,
                row,
                sub_col,
                sub_row,
            } => {
                format!(
                    "{}{}{}",
                    hints()[*col as usize],
                    hints()[*row as usize],
                    sub_hints()[(*sub_row * sub_cols() + *sub_col) as usize]
                )
            }
            _ => String::new(),
        }
    }
}

/// Converts a 2- or 3-character key string to a pixel position.
/// Returns None if the keys map to a position outside the current dynamic grid.
pub fn keys_to_pos(keys: &str, w: u32, h: u32) -> Option<(u32, u32)> {
    let hints = hints();
    let mut chars = keys.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    let col = hints.iter().position(|&c| c == c0)? as u32;
    let row = hints.iter().position(|&c| c == c1)? as u32;
    let ncols = dynamic_cols(w);
    let nrows = dynamic_rows(h);

    // Reject hints that map outside the currently rendered grid
    if col >= ncols || row >= nrows {
        return None;
    }

    let cell_w = w / ncols;
    let cell_h = h / nrows;
    match chars.next() {
        None => Some((col * cell_w + cell_w / 2, row * cell_h + cell_h / 2)),
        Some(c2) => {
            let idx = sub_hints().iter().position(|&c| c == c2)? as u32;
            let sub_col = idx % sub_cols();
            let sub_row = idx / sub_cols();
            let sub_cell_w = cell_w / sub_cols();
            let sub_cell_h = cell_h / sub_rows();
            Some((
                col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2,
                row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2,
            ))
        }
    }
}
