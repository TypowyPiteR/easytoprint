//! Runtime UI translations.
//!
//! English is embedded in the binary (lang/en.json at build time) and always
//! available. Any `*.json` file placed in the `lang/` folder next to the
//! program (or in the lib layout) adds a language — its entries override
//! the English base, so translation files may be partial.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const EMBEDDED_EN: &str = include_str!("../lang/en.json");

pub struct Lang {
    pub name: String,
    /// file name in lang/, e.g. "en.json" (for the config)
    pub file: String,
    strings: BTreeMap<String, String>,
}

fn parse(json: &str) -> Option<(String, BTreeMap<String, String>)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let mut strings = BTreeMap::new();
    if let Some(map) = v.get("strings").and_then(|s| s.as_object()) {
        for (k, val) in map {
            if let Some(text) = val.as_str() {
                strings.insert(k.clone(), text.to_string());
            }
        }
    }
    Some((name, strings))
}

impl Lang {
    /// English built into the binary — the guaranteed base.
    pub fn english() -> Lang {
        let (name, strings) = parse(EMBEDDED_EN).unwrap_or_default();
        Lang {
            name,
            file: "en.json".into(),
            strings,
        }
    }

    /// English base overlaid with the chosen language file (if present).
    pub fn load(dir: Option<&Path>, file: &str) -> Lang {
        let mut lang = Lang::english();
        lang.file = file.to_string();
        if let Some(dir) = dir {
            let path = dir.join(file);
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some((name, strings)) = parse(&text) {
                    lang.name = name;
                    for (k, v) in strings {
                        lang.strings.insert(k, v);
                    }
                }
            }
        }
        lang
    }

    /// Plain text lookup; missing keys fall back to the key itself
    /// (embedded English covers everything in practice).
    pub fn tr(&self, key: &str) -> String {
        self.strings
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }
}

/// All `*.json` files in the lang/ folder: (file name, display name).
pub fn available(dir: Option<&Path>) -> Vec<(String, String)> {
    let mut list = vec![("en.json".to_string(), Lang::english().name)];
    if let Some(dir) = dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    let file = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if file == "en.json" {
                        continue;
                    }
                    let name = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|t| parse(&t).map(|(n, _)| n))
                        .unwrap_or_else(|| file.clone());
                    list.push((file, name));
                }
            }
        }
    }
    list
}

/// The lang/ folder: next to the executable or in its parent folders.
pub fn lang_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut cur = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..4 {
        match cur {
            Some(dir) => {
                if dir.join("lang").is_dir() {
                    return Some(dir.join("lang"));
                }
                cur = dir.parent().map(|p| p.to_path_buf());
            }
            None => break,
        }
    }
    None
}
