use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::CreftError;

/// Persistent settings stored as JSON.
///
/// Settings are global (~/.creft/settings.json). Project-level settings
/// are not supported in v0.3.0 — all settings are user-scoped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(flatten)]
    values: BTreeMap<String, String>,
}

/// Known setting keys.
const KNOWN_KEYS: &[&str] = &["shell"];

impl Settings {
    /// Load settings from a JSON file, or return defaults if the file
    /// doesn't exist.
    pub fn load(path: &Path) -> Result<Self, CreftError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| CreftError::SettingsError(format!("invalid settings: {e}")))
    }

    /// Save settings to a JSON file.
    ///
    /// Creates parent directories if they don't exist.
    pub fn save(&self, path: &Path) -> Result<(), CreftError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| CreftError::SettingsError(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get a setting value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    /// Set a setting value. Returns an error for unknown keys.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), CreftError> {
        if !KNOWN_KEYS.contains(&key) {
            return Err(CreftError::SettingsError(format!(
                "unknown setting: '{key}'. Known settings: {}",
                KNOWN_KEYS.join(", ")
            )));
        }
        self.values.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Iterate over all settings as (key, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    // ── Settings::load() ─────────────────────────────────────────────────────

    #[test]
    fn load_returns_defaults_when_file_does_not_exist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.iter().count(), 0);
    }

    #[test]
    fn load_parses_valid_json_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"shell": "zsh"}"#).unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.get("shell"), Some("zsh"));
    }

    #[test]
    fn load_returns_error_for_malformed_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not json at all").unwrap();
        let result = Settings::load(&path);
        assert!(
            matches!(result, Err(CreftError::SettingsError(ref msg)) if msg.contains("invalid settings")),
            "expected SettingsError with 'invalid settings', got: {result:?}"
        );
    }

    // ── Settings::save() ─────────────────────────────────────────────────────

    #[test]
    fn save_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("dirs").join("settings.json");
        let settings = Settings::default();
        settings.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_and_load_round_trips_values() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let mut original = Settings::default();
        original.set("shell", "zsh").unwrap();
        original.save(&path).unwrap();

        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.get("shell"), Some("zsh"));
    }

    // ── Settings::set() ──────────────────────────────────────────────────────

    #[rstest]
    #[case::shell("shell", "zsh", true)]
    #[case::unknown_key("unknown-key", "value", false)]
    #[case::empty_key("", "value", false)]
    fn set_accepts_known_keys_and_rejects_unknown(
        #[case] key: &str,
        #[case] value: &str,
        #[case] should_succeed: bool,
    ) {
        let mut settings = Settings::default();
        let result = settings.set(key, value);
        assert_eq!(result.is_ok(), should_succeed);
    }

    #[test]
    fn set_unknown_key_error_lists_known_keys() {
        let mut settings = Settings::default();
        let err = settings.set("bad-key", "val").unwrap_err();
        match err {
            CreftError::SettingsError(msg) => {
                assert!(
                    msg.contains("unknown setting"),
                    "error should mention 'unknown setting': {msg}"
                );
                assert!(
                    msg.contains("shell"),
                    "error should list known settings: {msg}"
                );
            }
            other => panic!("expected SettingsError, got {other:?}"),
        }
    }

    #[test]
    fn set_none_value_is_stored_as_literal_none_string() {
        let mut settings = Settings::default();
        settings.set("shell", "none").unwrap();
        assert_eq!(settings.get("shell"), Some("none"));
    }

    // ── Settings::get() and iter() ───────────────────────────────────────────

    #[test]
    fn get_returns_none_for_missing_key() {
        let settings = Settings::default();
        assert_eq!(settings.get("shell"), None);
    }

    #[test]
    fn iter_returns_all_stored_pairs() {
        let mut settings = Settings::default();
        settings.set("shell", "zsh").unwrap();
        let pairs: Vec<_> = settings.iter().collect();
        assert_eq!(pairs, vec![("shell", "zsh")]);
    }

    #[test]
    fn iter_is_empty_on_default() {
        let settings = Settings::default();
        assert_eq!(settings.iter().count(), 0);
    }

    // ── round-trip JSON format ────────────────────────────────────────────────

    #[test]
    fn save_produces_pretty_json_with_shell_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::default();
        settings.set("shell", "zsh").unwrap();
        settings.save(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["shell"], "zsh");
    }
}
