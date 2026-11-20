//! `stochos --config`: edit common settings in a centered panel with a live
//! preview, and save the changed ones back to config.toml.

use crate::{
    backend::{Backend, KeyEvent},
    config::{color_from_hex, color_to_hex, config, config_path, Colors, Config},
    mode::{Mode, ModeTransition},
    render::{render_settings, SettingsRow},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Num(u32),
    Toggle(bool),
    Color([u8; 4]),
}

struct Field {
    label: &'static str,
    /// Key path in config.toml, e.g. `["colors", "border"]`.
    path: &'static [&'static str],
    read: fn(&Config) -> Value,
    /// The preview color this field sets, if it is a color.
    color: Option<fn(&mut Colors) -> &mut [u8; 4]>,
}

const NUM_RANGE: std::ops::RangeInclusive<u32> = 1..=10;
const HINT_SIZE: usize = 0;

const FIELDS: &[Field] = &[
    Field {
        label: "hint size",
        path: &["hint_font_size"],
        read: |c| Value::Num(c.hint_font_size()),
        color: None,
    },
    Field {
        label: "grid font size",
        path: &["font_size"],
        read: |c| Value::Num(c.font_size()),
        color: None,
    },
    Field {
        label: "hint auto click",
        path: &["hint", "auto_click"],
        read: |c| Value::Toggle(c.hint.auto_click),
        color: None,
    },
    Field {
        label: "hint text",
        path: &["colors", "hint_text"],
        read: |c| Value::Color(c.colors.hint_text),
        color: Some(|c| &mut c.hint_text),
    },
    Field {
        label: "hint typed text",
        path: &["colors", "hint_text_typed"],
        read: |c| Value::Color(c.colors.hint_text_typed),
        color: Some(|c| &mut c.hint_text_typed),
    },
    Field {
        label: "hint background",
        path: &["colors", "hint_chip_bg"],
        read: |c| Value::Color(c.colors.hint_chip_bg),
        color: Some(|c| &mut c.hint_chip_bg),
    },
    Field {
        label: "hint dimmed",
        path: &["colors", "hint_dim"],
        read: |c| Value::Color(c.colors.hint_dim),
        color: Some(|c| &mut c.hint_dim),
    },
    Field {
        label: "border",
        path: &["colors", "border"],
        read: |c| Value::Color(c.colors.border),
        color: Some(|c| &mut c.border),
    },
];

#[derive(Clone)]
pub struct SettingsState {
    values: Vec<Value>,
    saved: Vec<Value>,
    selected: usize,
    /// Hex digits typed so far while editing the selected color.
    editing: Option<Vec<char>>,
    status: Option<String>,
    confirm_quit: bool,
}

impl SettingsState {
    pub fn load() -> Self {
        let values: Vec<Value> = FIELDS.iter().map(|f| (f.read)(config())).collect();
        Self {
            saved: values.clone(),
            values,
            selected: 0,
            editing: None,
            status: None,
            confirm_quit: false,
        }
    }

    fn dirty(&self) -> bool {
        self.values != self.saved
    }

    fn adjust(&mut self, delta: i32) {
        match &mut self.values[self.selected] {
            Value::Num(n) => {
                let next = (*n as i32 + delta).max(0) as u32;
                *n = next.clamp(*NUM_RANGE.start(), *NUM_RANGE.end());
            }
            Value::Toggle(on) => *on = !*on,
            Value::Color(_) => {}
        }
    }
}

pub(super) fn handle_key(key: &KeyEvent, state: &SettingsState) -> anyhow::Result<ModeTransition> {
    let mut s = state.clone();
    s.status = None;
    s.confirm_quit = false;
    let ch = config().keys.char_of(key);

    if let Some(digits) = &mut s.editing {
        match key {
            KeyEvent::Close => s.editing = None,
            KeyEvent::Undo => {
                digits.pop();
            }
            KeyEvent::DoubleClick => {
                let hex: String = digits.iter().collect();
                match color_from_hex(&hex) {
                    Some(color) => {
                        s.values[s.selected] = Value::Color(color);
                        s.editing = None;
                    }
                    None => s.status = Some("type 6 or 8 hex digits".into()),
                }
            }
            _ => match ch {
                Some(c) if c.is_ascii_hexdigit() && digits.len() < 8 => {
                    digits.push(c.to_ascii_uppercase())
                }
                _ => return Ok(ModeTransition::Stay),
            },
        }
        return Ok(replace(s));
    }

    let rows = FIELDS.len();
    match (key, ch) {
        (KeyEvent::Close, _) => {
            if !state.dirty() || state.confirm_quit {
                return Ok(ModeTransition::Exit);
            }
            s.confirm_quit = true;
            s.status = Some("unsaved: esc again to quit, s to save".into());
        }
        (KeyEvent::ScrollUp, _) | (_, Some('k')) => s.selected = (s.selected + rows - 1) % rows,
        (KeyEvent::ScrollDown, _) | (_, Some('j')) => s.selected = (s.selected + 1) % rows,
        (KeyEvent::ScrollLeft, _) | (_, Some('h')) => s.adjust(-1),
        (KeyEvent::ScrollRight, _) | (_, Some('l')) => s.adjust(1),
        (KeyEvent::DoubleClick | KeyEvent::Click, _) => match s.values[s.selected] {
            Value::Color(_) => s.editing = Some(Vec::new()),
            Value::Toggle(_) => s.adjust(1),
            Value::Num(_) => return Ok(ModeTransition::Stay),
        },
        (_, Some('s')) => match save(&s) {
            Ok(()) => {
                s.saved = s.values.clone();
                s.status = Some("saved".into());
            }
            Err(e) => s.status = Some(format!("save failed: {e}")),
        },
        _ => return Ok(ModeTransition::Stay),
    }
    Ok(replace(s))
}

fn replace(s: SettingsState) -> ModeTransition {
    ModeTransition::Replace(Mode::Settings(Box::new(s)))
}

pub(super) fn draw<B: Backend>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    s: &SettingsState,
) -> anyhow::Result<()> {
    let rows: Vec<SettingsRow> = FIELDS
        .iter()
        .zip(&s.values)
        .map(|(f, v)| SettingsRow {
            label: f.label,
            value: match v {
                Value::Num(n) => format!("< {n} >"),
                Value::Toggle(on) => format!("< {} >", if *on { "on" } else { "off" }),
                Value::Color(c) => short_hex(*c),
            },
            swatch: match v {
                Value::Color(c) => Some(*c),
                _ => None,
            },
        })
        .collect();

    let mut colors = config().colors.clone();
    for (f, v) in FIELDS.iter().zip(&s.values) {
        if let (Some(field), Value::Color(c)) = (f.color, v) {
            *field(&mut colors) = *c;
        }
    }
    let hint_scale = match s.values[HINT_SIZE] {
        Value::Num(n) => n,
        _ => 1,
    };

    render_settings(
        pixels,
        width,
        height,
        &rows,
        s.selected,
        s.editing.as_deref(),
        s.status.as_deref(),
        &colors,
        hint_scale,
    );
    backend.present(pixels, width, height)
}

/// `#RRGGBB` for opaque colors, `#RRGGBBAA` otherwise.
fn short_hex(color: [u8; 4]) -> String {
    let hex = color_to_hex(color);
    if color[3] == 0xFF {
        hex[..7].to_string()
    } else {
        hex
    }
}

fn save(s: &SettingsState) -> anyhow::Result<()> {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let changed = FIELDS
        .iter()
        .zip(s.values.iter().zip(&s.saved))
        .filter(|(_, (now, before))| now != before)
        .map(|(f, (now, _))| (f.path, *now));
    let text = apply_edits(&text, changed)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, text)?;
    Ok(())
}

/// Sets each `(path, value)` in a config.toml document, keeping everything
/// else in it — comments, order and keys this version doesn't know.
fn apply_edits<'a>(
    text: &str,
    edits: impl Iterator<Item = (&'a [&'a str], Value)>,
) -> anyhow::Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    for (path, value) in edits {
        let item = match value {
            Value::Num(n) => toml_edit::value(i64::from(n)),
            Value::Toggle(on) => toml_edit::value(on),
            Value::Color(c) => toml_edit::value(color_to_hex(c)),
        };
        match path {
            [key] => doc[key] = item,
            [table, key] => doc[table][key] = item,
            _ => anyhow::bail!("unsupported config path {path:?}"),
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments_and_unknown_keys() {
        let text = "# mine\nfont = \"x.ttf\"\nfont_size = 2\n\n[colors]\nborder = \"#FFA500FF\" # orange\n";
        let edits = [
            (&["font_size"][..], Value::Num(3)),
            (&["hint_font_size"][..], Value::Num(2)),
            (
                &["colors", "hint_text"][..],
                Value::Color([0x7A, 0xB0, 0xFF, 0xFF]),
            ),
            (&["hint", "auto_click"][..], Value::Toggle(true)),
        ];
        let out = apply_edits(text, edits.into_iter()).unwrap();
        assert!(
            out.starts_with("# mine\nfont = \"x.ttf\"\nfont_size = 3\n"),
            "{out}"
        );
        assert!(out.contains("border = \"#FFA500FF\" # orange"), "{out}");
        let parsed: Config = toml::from_str(&out).unwrap();
        assert_eq!(parsed.font_size, 3);
        assert_eq!(parsed.hint_font_size(), 2);
        assert_eq!(parsed.colors.hint_text, [0x7A, 0xB0, 0xFF, 0xFF]);
        assert!(parsed.hint.auto_click);
    }

    #[test]
    fn fields_point_at_real_config_keys() {
        // A non-default value written at each field's path must read back.
        let changed = |v: Value| match v {
            Value::Num(n) => Value::Num(n % 10 + 1),
            Value::Toggle(on) => Value::Toggle(!on),
            Value::Color([b, g, r, a]) => Value::Color([!b, g, r, a]),
        };
        let config = Config::default();
        let edits = FIELDS.iter().map(|f| (f.path, changed((f.read)(&config))));
        let out = apply_edits("", edits).unwrap();
        let parsed: Config = toml::from_str(&out).unwrap();
        for f in FIELDS {
            assert_eq!((f.read)(&parsed), changed((f.read)(&config)), "{}", f.label);
        }
    }
}
