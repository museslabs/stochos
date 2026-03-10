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

/// Maps a Wayland key code to an ASCII character.
pub fn keycode_to_char(kc: u32) -> Option<char> {
    match kc {
        2 => Some('1'),
        3 => Some('2'),
        4 => Some('3'),
        5 => Some('4'),
        6 => Some('5'),
        7 => Some('6'),
        8 => Some('7'),
        9 => Some('8'),
        10 => Some('9'),
        11 => Some('0'),
        16 => Some('q'),
        17 => Some('w'),
        18 => Some('e'),
        19 => Some('r'),
        20 => Some('t'),
        21 => Some('y'),
        22 => Some('u'),
        23 => Some('i'),
        24 => Some('o'),
        25 => Some('p'),
        30 => Some('a'),
        31 => Some('s'),
        32 => Some('d'),
        33 => Some('f'),
        34 => Some('g'),
        35 => Some('h'),
        36 => Some('j'),
        37 => Some('k'),
        38 => Some('l'),
        39 => Some(';'),
        44 => Some('z'),
        45 => Some('x'),
        46 => Some('c'),
        47 => Some('v'),
        48 => Some('b'),
        49 => Some('n'),
        50 => Some('m'),
        52 => Some('.'),
        53 => Some('/'),
        41 => Some('`'),
        _ => None,
    }
}
