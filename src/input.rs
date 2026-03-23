/// Main grid: 20 home-row-biased hint chars → 20×20 = 400 cells
pub const HINTS: &[char] = &[
    'a', 's', 'd', 'f', 'j', 'k', 'l', ';', 'g', 'h', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o',
    'p',
];
pub const COLS: u32 = HINTS.len() as u32;
pub const ROWS: u32 = HINTS.len() as u32;

/// Sub-grid: 25 unique chars laid out in a 5×5 grid (single keypress selects a cell).
/// Uses a broader set than HINTS so all 25 slots can be filled.
pub const SUB_HINTS: &[char] = &[
    'a', 's', 'd', 'f', 'j', 'k', 'l', ';', 'g', 'h', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o',
    'p', 'z', 'x', 'c', 'v', 'b',
];
pub const SUB_COLS: u32 = 5;
pub const SUB_ROWS: u32 = 5;

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
                format!("{}{}", HINTS[*col as usize], HINTS[*row as usize])
            }
            InputState::Ready {
                col,
                row,
                sub_col,
                sub_row,
            } => {
                format!(
                    "{}{}{}",
                    HINTS[*col as usize],
                    HINTS[*row as usize],
                    SUB_HINTS[(*sub_row * SUB_COLS + *sub_col) as usize]
                )
            }
            _ => String::new(),
        }
    }
}

/// Converts a 2- or 3-character key string to a pixel position.
pub fn keys_to_pos(keys: &str, w: u32, h: u32) -> Option<(u32, u32)> {
    let mut chars = keys.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    let col = HINTS.iter().position(|&c| c == c0)? as u32;
    let row = HINTS.iter().position(|&c| c == c1)? as u32;
    let cell_w = w / COLS;
    let cell_h = h / ROWS;
    match chars.next() {
        None => Some((col * cell_w + cell_w / 2, row * cell_h + cell_h / 2)),
        Some(c2) => {
            let idx = SUB_HINTS.iter().position(|&c| c == c2)? as u32;
            let sub_col = idx % SUB_COLS;
            let sub_row = idx / SUB_COLS;
            let sub_cell_w = cell_w / SUB_COLS;
            let sub_cell_h = cell_h / SUB_ROWS;
            Some((
                col * cell_w + sub_col * sub_cell_w + sub_cell_w / 2,
                row * cell_h + sub_row * sub_cell_h + sub_cell_h / 2,
            ))
        }
    }
}
