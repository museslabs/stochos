use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone)]
pub enum MacroActionKind {
    Move(String),
    Click(String),
    DoubleClick(String),
    TripleClick(String),
    RightClick(String),
    MiddleClick(String),
    Drag(String, String),
}

#[derive(Serialize, Clone)]
pub struct MacroAction {
    pub kind: MacroActionKind,
    #[serde(skip_serializing_if = "is_zero")]
    pub wait_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl MacroAction {
    pub fn new(kind: MacroActionKind, wait_ms: u64) -> Self {
        Self { kind, wait_ms }
    }
}

impl<'de> Deserialize<'de> for MacroAction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        // Buffer into a Value and dispatch on shape by hand. A `#[serde(untagged)]`
        // enum here swallows the real cause (e.g. an unknown action kind) behind
        // "data did not match any variant of untagged enum Repr".
        let value = serde_json::Value::deserialize(deserializer)?;
        // The new shape carries a `kind` field; the legacy shape is the bare
        // externally-tagged action kind (e.g. `{ "Click": "as" }`) from older files.
        if value.get("kind").is_some() {
            #[derive(Deserialize)]
            struct New {
                kind: MacroActionKind,
                #[serde(default)]
                wait_ms: u64,
            }
            let New { kind, wait_ms } = serde_json::from_value(value).map_err(D::Error::custom)?;
            Ok(MacroAction { kind, wait_ms })
        } else {
            let kind = serde_json::from_value(value).map_err(D::Error::custom)?;
            Ok(MacroAction { kind, wait_ms: 0 })
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MacroEntry {
    pub name: String,
    pub actions: Vec<MacroAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_key: Option<char>,
}

pub struct MacroStore {
    pub macros: Vec<MacroEntry>,
    path: PathBuf,
}

impl MacroStore {
    /// Best-effort load for everything except `--list-macros`: unparseable
    /// entries are skipped with a warning and a malformed file yields an empty
    /// store, so launching the overlay never fails over one bad macro.
    pub fn load() -> Self {
        let path = config_path();
        let macros = match read_entries(&path) {
            Ok((macros, errors)) => {
                for err in &errors {
                    eprintln!("stochos: skipping {err}");
                }
                macros
            }
            Err(e) => {
                eprintln!("stochos: ignoring macros at {}: {e:#}", path.display());
                Vec::new()
            }
        };
        MacroStore { macros, path }
    }

    /// Strict load for `--list-macros`: a present-but-unparseable file is a hard
    /// error that names the offending macro, instead of silently hiding it.
    pub fn load_strict() -> anyhow::Result<Self> {
        let path = config_path();
        let (macros, errors) = read_entries(&path)?;
        if !errors.is_empty() {
            anyhow::bail!("{}", errors.join("\n"));
        }
        Ok(MacroStore { macros, path })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.macros)?;
        fs::write(&self.path, data)?;
        Ok(())
    }

    pub fn find_by_key(&self, key: char) -> Option<&MacroEntry> {
        self.macros.iter().find(|m| m.bind_key == Some(key))
    }

    pub fn fuzzy_search(&self, query: &[char]) -> Vec<&MacroEntry> {
        if query.is_empty() {
            return self.macros.iter().collect();
        }
        let query_str: String = query.iter().map(|c| c.to_ascii_lowercase()).collect();
        let mut entries: Vec<(&MacroEntry, String)> = self
            .macros
            .iter()
            .map(|m| (m, m.name.to_lowercase()))
            .collect();
        entries.retain(|(_, lower)| fuzzy_match(lower, &query_str));
        entries.sort_by(|(_, a_lower), (_, b_lower)| {
            fuzzy_score(b_lower, &query_str).cmp(&fuzzy_score(a_lower, &query_str))
        });
        entries.into_iter().map(|(m, _)| m).collect()
    }

    pub fn add(&mut self, entry: MacroEntry) {
        if let Some(key) = entry.bind_key {
            self.macros.retain(|m| m.bind_key != Some(key));
        }
        self.macros.push(entry);
    }
}

/// Parse macros.json into (valid entries, per-entry error messages). The outer
/// error covers an unreadable file or invalid JSON syntax; a missing file is
/// not an error (empty store). Parsing entry-by-entry lets a bad macro name
/// itself rather than failing the whole file with a misleading position.
fn read_entries(path: &Path) -> anyhow::Result<(Vec<MacroEntry>, Vec<String>)> {
    use anyhow::Context as _;
    let data = match fs::read_to_string(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("failed to read macros at {}", path.display())))
        }
    };
    let raw: Vec<serde_json::Value> = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse macros at {}", path.display()))?;
    let mut macros = Vec::with_capacity(raw.len());
    let mut errors = Vec::new();
    for (i, entry) in raw.into_iter().enumerate() {
        let label = entry
            .get("name")
            .and_then(|v| v.as_str())
            .map_or_else(|| "<unnamed>".to_string(), |n| format!("\"{n}\""));
        match serde_json::from_value::<MacroEntry>(entry) {
            Ok(m) => macros.push(m),
            Err(e) => errors.push(format!("macro #{} ({label}): {e}", i + 1)),
        }
    }
    Ok((macros, errors))
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
        .join("macros.json")
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let mut chars = needle.chars();
    let mut current = chars.next();
    for h in haystack.chars() {
        if let Some(c) = current {
            if h == c {
                current = chars.next();
            }
        } else {
            return true;
        }
    }
    current.is_none()
}

fn fuzzy_score(haystack: &str, needle: &str) -> i32 {
    if haystack.starts_with(needle) {
        return 100;
    }
    if haystack.contains(needle) {
        return 50;
    }
    let mut score = 0;
    let mut needle_chars = needle.chars();
    let mut current = needle_chars.next();
    let mut gap = 0;
    for h in haystack.chars() {
        if let Some(c) = current {
            if h == c {
                score += 10 - gap.min(9);
                gap = 0;
                current = needle_chars.next();
            } else {
                gap += 1;
            }
        }
    }
    score
}
