use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::backend::KeyEvent;

static CONFIG: OnceLock<Config> = OnceLock::new();
const SCALE_RANGE: std::ops::RangeInclusive<u32> = 1..=10;

pub fn init() {
    CONFIG.set(Config::load()).ok();
}

pub fn config() -> &'static Config {
    CONFIG.get().expect("config not initialized")
}

pub fn colors() -> &'static Colors {
    &config().colors
}

pub fn font_size() -> u32 {
    config().font_size()
}

pub fn sub_hint_font_size() -> u32 {
    config().sub_hint_font_size()
}

pub fn panel_font_size() -> u32 {
    config().panel_font_size()
}

fn clamp_scale(scale: u32) -> u32 {
    scale.clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end())
}

/// Platform-agnostic key representation.
/// Each backend maps its native keycodes to these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    // Whitespace / editing
    Space,
    Enter,
    Escape,
    Backspace,
    Tab,
    // Navigation
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Lock / toggle keys
    CapsLock,
    NumLock,
    ScrollLock,
    // System keys
    PrintScreen,
    Pause,
    ContextMenu,
    // Numpad
    NumPad0,
    NumPad1,
    NumPad2,
    NumPad3,
    NumPad4,
    NumPad5,
    NumPad6,
    NumPad7,
    NumPad8,
    NumPad9,
    NumPadAdd,
    NumPadSubtract,
    NumPadMultiply,
    NumPadDivide,
    NumPadDecimal,
    NumPadEnter,
}

/// (name, Key) pairs for all non-Char variants, used by serde.
const SPECIAL_KEYS: &[(&str, Key)] = &[
    ("space", Key::Space),
    ("enter", Key::Enter),
    ("escape", Key::Escape),
    ("backspace", Key::Backspace),
    ("tab", Key::Tab),
    ("insert", Key::Insert),
    ("delete", Key::Delete),
    ("home", Key::Home),
    ("end", Key::End),
    ("page_up", Key::PageUp),
    ("page_down", Key::PageDown),
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("f1", Key::F1),
    ("f2", Key::F2),
    ("f3", Key::F3),
    ("f4", Key::F4),
    ("f5", Key::F5),
    ("f6", Key::F6),
    ("f7", Key::F7),
    ("f8", Key::F8),
    ("f9", Key::F9),
    ("f10", Key::F10),
    ("f11", Key::F11),
    ("f12", Key::F12),
    ("caps_lock", Key::CapsLock),
    ("num_lock", Key::NumLock),
    ("scroll_lock", Key::ScrollLock),
    ("print_screen", Key::PrintScreen),
    ("pause", Key::Pause),
    ("context_menu", Key::ContextMenu),
    ("num_pad_0", Key::NumPad0),
    ("num_pad_1", Key::NumPad1),
    ("num_pad_2", Key::NumPad2),
    ("num_pad_3", Key::NumPad3),
    ("num_pad_4", Key::NumPad4),
    ("num_pad_5", Key::NumPad5),
    ("num_pad_6", Key::NumPad6),
    ("num_pad_7", Key::NumPad7),
    ("num_pad_8", Key::NumPad8),
    ("num_pad_9", Key::NumPad9),
    ("num_pad_add", Key::NumPadAdd),
    ("num_pad_subtract", Key::NumPadSubtract),
    ("num_pad_multiply", Key::NumPadMultiply),
    ("num_pad_divide", Key::NumPadDivide),
    ("num_pad_decimal", Key::NumPadDecimal),
    ("num_pad_enter", Key::NumPadEnter),
];

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if let Key::Char(c) = self {
            return s.serialize_str(&c.to_string());
        }
        for &(name, ref key) in SPECIAL_KEYS {
            if key == self {
                return s.serialize_str(name);
            }
        }
        unreachable!()
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        for &(name, key) in SPECIAL_KEYS {
            if s == name {
                return Ok(key);
            }
        }
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(Key::Char(c)),
            _ => Err(serde::de::Error::custom(format!("unknown key: {s}"))),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GridConfig {
    pub hints: Vec<char>,
    pub sub_hints: Vec<char>,
    pub sub_cols: u32,
    /// Target cell size in pixels for dynamic grid. If set (non-zero), enables dynamic sizing.
    pub target_cell_size: Option<u32>,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            hints: vec![
                'a', 's', 'd', 'f', 'j', 'k', 'l', ';', 'g', 'h', 'q', 'w', 'e', 'r', 't', 'y',
                'u', 'i', 'o', 'p',
            ],
            sub_hints: vec![
                'a', 's', 'd', 'f', 'j', 'k', 'l', ';', 'g', 'h', 'q', 'w', 'e', 'r', 't', 'y',
                'u', 'i', 'o', 'p', 'z', 'x', 'c', 'v', 'b',
            ],
            sub_cols: 5,
            target_cell_size: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    pub click: Key,
    pub double_click: Key,
    pub close: Key,
    pub undo: Key,
    pub macro_menu: Key,
    pub macro_record: Key,
    pub right_click: Key,
    pub scroll_up: Key,
    pub scroll_down: Key,
    pub scroll_left: Key,
    pub scroll_right: Key,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            click: Key::Space,
            double_click: Key::Enter,
            close: Key::Escape,
            undo: Key::Backspace,
            macro_menu: Key::Tab,
            macro_record: Key::Char('`'),
            right_click: Key::Delete,
            scroll_up: Key::Up,
            scroll_down: Key::Down,
            scroll_left: Key::Left,
            scroll_right: Key::Right,
        }
    }
}

impl KeyBindings {
    /// Look up whether a Key is bound to an action. Returns the corresponding
    /// KeyEvent if bound, or None if the key is not an action binding.
    pub fn to_event(&self, key: Key) -> Option<KeyEvent> {
        if key == self.click {
            return Some(KeyEvent::Click);
        }
        if key == self.double_click {
            return Some(KeyEvent::DoubleClick);
        }
        if key == self.close {
            return Some(KeyEvent::Close);
        }
        if key == self.undo {
            return Some(KeyEvent::Undo);
        }
        if key == self.macro_menu {
            return Some(KeyEvent::MacroMenu);
        }
        if key == self.macro_record {
            return Some(KeyEvent::MacroRecord);
        }
        if key == self.right_click {
            return Some(KeyEvent::RightClick);
        }
        if key == self.scroll_up {
            return Some(KeyEvent::ScrollUp);
        }
        if key == self.scroll_down {
            return Some(KeyEvent::ScrollDown);
        }
        if key == self.scroll_left {
            return Some(KeyEvent::ScrollLeft);
        }
        if key == self.scroll_right {
            return Some(KeyEvent::ScrollRight);
        }
        None
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
///Colors stored as BGRA
pub struct Colors {
    #[serde(with = "from_hex")]
    pub cell_normal: [u8; 4],
    #[serde(with = "from_hex")]
    pub cell_drag: [u8; 4],
    #[serde(with = "from_hex")]
    pub cell_highlight: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_first: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_second: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_highlight: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_dim: [u8; 4],

    // // Sub-grid
    #[serde(with = "from_hex")]
    pub sub_cell_normal: [u8; 4],
    #[serde(with = "from_hex")]
    pub sub_bg: [u8; 4],
    // // Macro UI
    #[serde(with = "from_hex")]
    pub panel_bg: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_white: [u8; 4],
    #[serde(with = "from_hex")]
    pub text_grey: [u8; 4],
    #[serde(with = "from_hex")]
    pub selected_bg: [u8; 4],
    #[serde(with = "from_hex")]
    pub rec_bg: [u8; 4],

    #[serde(with = "from_hex")]
    pub border: [u8; 4],
    #[serde(with = "from_hex")]
    pub border_dragging: [u8; 4],
}
mod from_hex {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(color: &[u8; 4], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            color[2], color[1], color[0], color[3]
        ))
    }
    ///Deserialize from #RRGGBBAA  to [BB,GG,RR,AA]
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 4], D::Error> {
        let s = String::deserialize(deserializer)?;
        let hex = s.trim_start_matches("#");
        let rgba = match hex.len() {
            6 => u32::from_str_radix(&hex, 16)
                .map(|a| a << 8)
                .unwrap_or_default(),
            8 => u32::from_str_radix(&hex, 16).unwrap_or_default(),
            _ => 0,
        };
        Ok([
            ((rgba & 0x0000FF00) >> 8) as u8,
            ((rgba & 0x00FF0000) >> 16) as u8,
            ((rgba & 0xFF000000) >> 24) as u8,
            (rgba & 0x000000FF) as u8,
        ])
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            //store in BGRA
            cell_normal: [0x00, 0x00, 0x00, 0x66], //overlay transparency
            cell_drag: [0x40, 0x00, 0x40, 0x88],   //dark purple
            cell_highlight: [0x14, 0x30, 0x14, 0xAA], //dark green
            text_first: [0x00, 0xDC, 0xFF, 0xFF],  //yellow
            text_second: [0xFF, 0xBE, 0x50, 0xFF], //sky blue
            text_highlight: [0x50, 0xFF, 0x50, 0xFF], //bright lime
            text_dim: [0x66, 0x66, 0x66, 0xAA],

            // // Sub-grid
            sub_cell_normal: [0x30, 0x10, 0x00, 0xAA], //dark navy
            sub_bg: [0x30, 0x10, 0x00, 0x99],

            // // Macro UI
            panel_bg: [0x18, 0x0C, 0x00, 0xEE], //very dark warm
            text_white: [0xFF, 0xFF, 0xFF, 0xFF],
            text_grey: [0x88, 0x88, 0x88, 0xFF],
            selected_bg: [0x30, 0x50, 0x20, 0xFF], //dark green
            rec_bg: [0x00, 0x00, 0xCC, 0xFF],
            border: [0x00, 0xA5, 0xFF, 0xFF],          //amber
            border_dragging: [0xFF, 0x00, 0xFF, 0xFF], //magenta
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub grid: GridConfig,
    pub keys: KeyBindings,
    pub colors: Colors,
    pub font_size: u32,
    pub sub_hint_font_size: Option<u32>,
    pub panel_font_size: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grid: GridConfig::default(),
            keys: KeyBindings::default(),
            colors: Colors::default(),
            font_size: 2,
            sub_hint_font_size: None,
            panel_font_size: None,
        }
    }
}

impl Config {
    fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(data) => toml::from_str(&data).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn font_size(&self) -> u32 {
        clamp_scale(self.font_size)
    }

    pub fn sub_hint_font_size(&self) -> u32 {
        self.sub_hint_font_size
            .map_or_else(|| self.font_size(), clamp_scale)
    }

    pub fn panel_font_size(&self) -> u32 {
        self.panel_font_size
            .or(self.sub_hint_font_size)
            .map_or_else(|| self.font_size(), clamp_scale)
    }

    pub fn cols(&self) -> u32 {
        self.grid.hints.len() as u32
    }

    pub fn rows(&self) -> u32 {
        self.grid.hints.len() as u32
    }

    pub fn sub_rows(&self) -> u32 {
        self.grid.sub_hints.len() as u32 / self.grid.sub_cols
    }

    /// Calculate dynamic grid columns based on screen width
    pub fn dynamic_cols(&self, screen_width: u32) -> u32 {
        if let Some(target) = self.grid.target_cell_size {
            if target > 0 {
                let calculated = (screen_width / target)
                    .max(2)
                    .min(self.grid.hints.len() as u32);
                return calculated;
            }
        }
        self.cols()
    }

    /// Calculate dynamic grid rows based on screen height
    pub fn dynamic_rows(&self, screen_height: u32) -> u32 {
        if let Some(target) = self.grid.target_cell_size {
            if target > 0 {
                let calculated = (screen_height / target)
                    .max(2)
                    .min(self.grid.hints.len() as u32);
                return calculated;
            }
        }
        self.rows()
    }
}

fn config_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from(".config"))
        })
        .join("stochos")
        .join("config.toml")
}
