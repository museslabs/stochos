use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub enum MacroAction {
    Move(String),
    Click(String),
    DoubleClick(String),
    Drag(String, String),
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
    pub fn load() -> Self {
        let path = config_path();
        let macros = fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default();
        MacroStore { macros, path }
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
