//! Persisted user settings — a small JSON file next to the executable
//! (`easytoprint.json`). Portable-app style: no registry, no AppData.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Config {
    /// language file name in lang/, "" = English
    #[serde(default)]
    pub language: String,
    /// printer name, "" = Windows default
    #[serde(default)]
    pub printer: String,
    #[serde(default)]
    pub autostart: bool,
    /// "printer" | "simplex" | "long" | "short"
    #[serde(default)]
    pub duplex: String,
    /// "printer" | "color" | "gray"
    #[serde(default)]
    pub color: String,
    /// paper code, e.g. "A4"; "" = printer default
    #[serde(default)]
    pub paper: String,
}

impl Config {
    pub fn load(path: &std::path::Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Config file location: next to the executable when the folder is
    /// writable (portable layout), otherwise %APPDATA%\EasyToPrint
    /// (installed under Program Files).
    pub fn path() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.to_path_buf();
        let probe = dir.join(".etp-write-test");
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                Some(dir.join("easytoprint.json"))
            }
            Err(_) => {
                let base = std::env::var("APPDATA")
                    .ok()
                    .map(|a| std::path::PathBuf::from(a).join("EasyToPrint"))?;
                let _ = std::fs::create_dir_all(&base);
                Some(base.join("easytoprint.json"))
            }
        }
    }
}
