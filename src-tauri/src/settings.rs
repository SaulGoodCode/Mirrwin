//! Settings that survive a restart.
//!
//! Everything the settings dialog can change lives here and is mirrored into
//! `AppState` at startup. The file sits in the OS app-config directory
//! (`%APPDATA%\com.mirrwin.app\settings.json` on Windows) rather than next to
//! the executable, so an install under Program Files still works without
//! elevation.
//!
//! Reads never fail loudly: a missing, unreadable or half-written file just
//! yields defaults, because losing preferences must not stop the app starting.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_name: String,
    pub port: u16,
    pub save_dir: String,
    pub enable_audio: bool,
    /// Requested capture size / rate (0 = let the library decide).
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: "AirPlay Mirror".to_string(),
            port: 7000,
            save_dir: String::new(),
            enable_audio: false,
            width: 0,
            height: 0,
            fps: 0,
        }
    }
}

fn file_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(FILE_NAME))
}

/// Read the saved settings, falling back to defaults for anything missing.
pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = file_path(app) else {
        return Settings::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Settings::default();
    };
    match parse(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[settings] ignoring unreadable {}: {e}", path.display());
            Settings::default()
        }
    }
}

/// Split out from `load` so the schema-compatibility rules can be tested
/// without an `AppHandle`.
///
/// `#[serde(default)]` on the struct is what lets a file written by an older
/// version still load: fields it never heard of simply take their default, and
/// fields it no longer knows are ignored.
fn parse(text: &str) -> Result<Settings, serde_json::Error> {
    serde_json::from_str(text)
}

/// Write the settings out. Errors are logged, not propagated: failing to save a
/// preference should never break the action the user actually asked for.
pub fn save(app: &AppHandle, settings: &Settings) {
    let Some(path) = file_path(app) else {
        eprintln!("[settings] no app config directory, not saving");
        return;
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("[settings] cannot create {}: {e}", dir.display());
            return;
        }
    }
    let json = match serde_json::to_string_pretty(settings) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[settings] cannot serialize: {e}");
            return;
        }
    };
    // Write beside the target and rename over it, so an interrupted write
    // leaves the previous settings intact rather than a truncated file.
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json) {
        eprintln!("[settings] cannot write {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        eprintln!("[settings] cannot replace {}: {e}", path.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut s = Settings::default();
        s.device_name = "客厅电视".to_string();
        s.port = 7100;
        s.enable_audio = true;
        s.width = 1280;
        let json = serde_json::to_string(&s).unwrap();
        let back = parse(&json).unwrap();
        assert_eq!(back.device_name, "客厅电视");
        assert_eq!(back.port, 7100);
        assert!(back.enable_audio);
        assert_eq!(back.width, 1280);
    }

    #[test]
    fn a_file_from_an_older_version_still_loads() {
        // Written before audio and the size fields existed.
        let old = r#"{"deviceName":"AirPlay Mirror","port":7000,"saveDir":"D:\\shots"}"#;
        let s = parse(old).expect("older settings must not be rejected");
        assert_eq!(s.save_dir, "D:\\shots");
        assert!(!s.enable_audio, "a missing field must take its default");
        assert_eq!(s.fps, 0);
    }

    #[test]
    fn a_file_from_a_newer_version_still_loads() {
        let newer = r#"{"port":7005,"somethingWeDoNotKnowYet":42}"#;
        let s = parse(newer).expect("unknown fields must be ignored, not fatal");
        assert_eq!(s.port, 7005);
        assert_eq!(s.device_name, Settings::default().device_name);
    }

    #[test]
    fn garbage_is_rejected_so_the_caller_can_fall_back() {
        assert!(parse("not json at all").is_err());
    }
}
