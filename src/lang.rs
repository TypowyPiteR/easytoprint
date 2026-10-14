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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the UI asks for must exist in the embedded English base.
    const UI_KEYS: &[&str] = &[
        "files.one", "files.many",
        "drop.main", "drop.sub", "drop.active", "drop.add",
        "btn.print", "btn.printN", "btn.printing", "btn.stop", "btn.clear",
        "warn.engine",
        "progress.sent", "progress.errors", "progress.now",
        "queue.empty.title", "queue.empty.sub", "queue.col.file", "queue.col.status",
        "status.waiting", "status.printing", "status.sent", "status.error",
        "status.sent.tip", "status.error.tip",
        "settings.title", "settings.group.general", "settings.group.printing",
        "settings.language", "settings.printer", "settings.printer.default",
        "settings.printer.defaultNamed", "settings.nextJob", "settings.autostart",
        "settings.duplex", "settings.color", "settings.paper", "settings.paper.default",
        "settings.paper.auto",
        "settings.legacyNote", "settings.close", "settings.tip",
        "duplex.printer", "duplex.simplex", "duplex.long", "duplex.short",
        "color.printer", "color.color", "color.grayscale",
    ];

    #[test]
    fn embedded_english_covers_all_ui_keys() {
        let en = Lang::english();
        for key in UI_KEYS {
            assert_ne!(&en.tr(key), key, "missing key in embedded English: {key}");
        }
    }

    #[test]
    fn language_files_override_and_fall_back() {
        // a language file overlays the embedded English base: overridden keys
        // use the translation, everything else falls back to English
        let mut lang = Lang::english();
        lang.strings.insert("drop.main".into(), "PRZECIĄGNIJ".into());
        assert_eq!(lang.tr("drop.main"), "PRZECIĄGNIJ");
        assert_eq!(lang.tr("btn.print"), "Print");
    }

    #[test]
    fn shipped_language_files_are_valid() {
        let en = Lang::english();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lang");
        let mut count = 0;
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            if !path.extension().map(|e| e == "json").unwrap_or(false) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let v: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
            assert!(
                v.get("name").and_then(|n| n.as_str()).is_some(),
                "{}: missing \"name\"",
                path.display()
            );
            if let Some(obj) = v.get("strings").and_then(|s| s.as_object()) {
                assert!(!obj.is_empty(), "{}: no strings", path.display());
                for k in obj.keys() {
                    assert_ne!(
                        en.tr(k),
                        k.as_str(),
                        "{}: unknown key {k} (typo? not in the English base)",
                        path.display()
                    );
                }
            }
            count += 1;
        }
        // all 24 official EU languages ship with the app
        assert!(count >= 24, "expected 24 EU language files, found {count}");
    }
}
