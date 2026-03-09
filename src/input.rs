/// Main grid: 20 home-row-biased hint chars → 20×20 = 400 cells
pub const HINTS: &[u8] = b"asdfjkl;ghqwertyuiop";
pub const COLS: u32 = HINTS.len() as u32;
pub const ROWS: u32 = HINTS.len() as u32;

/// Sub-grid: 25 unique chars laid out in a 5×5 grid (single keypress selects a cell).
/// Uses a broader set than HINTS so all 25 slots can be filled.
pub const SUB_HINTS: &[u8] = b"asdfjkl;ghqwertyuiopzxcvb";
pub const SUB_COLS: u32 = 5;
pub const SUB_ROWS: u32 = 5;

#[derive(Clone)]
pub enum InputState {
    /// Waiting for the first main-grid character
    First,
    /// First main-grid character pressed; waiting for second
    Second(u8),
    /// Main cell chosen; waiting for a single sub-grid character
    SubFirst { col: u32, row: u32 },
    /// Sub-cell chosen; mouse positioned, waiting for Space
    Ready,
}

/// Maps a Wayland key code to an ASCII character.
pub fn keycode_to_char(kc: u32) -> Option<u8> {
    match kc {
        16 => Some(b'q'),
        17 => Some(b'w'),
        18 => Some(b'e'),
        19 => Some(b'r'),
        20 => Some(b't'),
        21 => Some(b'y'),
        22 => Some(b'u'),
        23 => Some(b'i'),
        24 => Some(b'o'),
        25 => Some(b'p'),
        30 => Some(b'a'),
        31 => Some(b's'),
        32 => Some(b'd'),
        33 => Some(b'f'),
        34 => Some(b'g'),
        35 => Some(b'h'),
        36 => Some(b'j'),
        37 => Some(b'k'),
        38 => Some(b'l'),
        39 => Some(b';'),
        44 => Some(b'z'),
        45 => Some(b'x'),
        46 => Some(b'c'),
        47 => Some(b'v'),
        48 => Some(b'b'),
        49 => Some(b'n'),
        50 => Some(b'm'),
        _ => None,
    }
}

pub fn keycode_to_hint(kc: u32) -> Option<u8> {
    let ch = keycode_to_char(kc)?;
    HINTS.contains(&ch).then_some(ch)
}
