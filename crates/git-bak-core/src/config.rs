use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::types::WatchMode;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub workspace: PathBuf,
    #[serde(default = "default_watch_patterns")]
    pub watch: Vec<String>,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_push_interval_sec")]
    pub push_interval_sec: u64,
    #[serde(default = "default_storm_window_sec")]
    pub storm_window_sec: u64,
    #[serde(default)]
    pub mode: WatchMode,
}

impl WorkspaceConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let config_path = path.as_ref();
        let content = fs::read_to_string(config_path).map_err(|source| {
            Error::Config(format!(
                "failed to read config at {}: {source}",
                config_path.display()
            ))
        })?;

        Self::from_toml_str(&content).map_err(|source| {
            Error::Config(format!(
                "failed to parse config at {}: {}",
                config_path.display(),
                match source {
                    Error::Config(message) => message,
                    _ => source.to_string(),
                }
            ))
        })
    }

    pub fn from_toml_str(content: &str) -> Result<Self> {
        toml::from_str::<Self>(content)
            .map_err(|source| Error::Config(format!("invalid config format: {source}")))
    }
}

fn default_watch_patterns() -> Vec<String> {
    vec![
        "SOUL.md".to_owned(),
        "AGENTS.md".to_owned(),
        "USER.md".to_owned(),
        "IDENTITY.md".to_owned(),
        "TOOLS.md".to_owned(),
        "MEMORY.md".to_owned(),
        "memory/*.md".to_owned(),
    ]
}

fn default_debounce_ms() -> u64 {
    1_500
}

fn default_push_interval_sec() -> u64 {
    600
}

fn default_storm_window_sec() -> u64 {
    5
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::WorkspaceConfig;
    use crate::types::WatchMode;

    #[test]
    fn test_defaults_apply_to_optional_fields() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(err) => panic!("failed to create tempdir: {err}"),
        };
        let config_path = dir.path().join("personaguard.toml");

        let write_result = fs::write(&config_path, "workspace = \"/tmp/repo\"\n");
        assert!(write_result.is_ok());

        let config = match WorkspaceConfig::load(config_path) {
            Ok(config) => config,
            Err(err) => panic!("failed to load config: {err}"),
        };

        assert_eq!(config.workspace, PathBuf::from("/tmp/repo"));
        assert_eq!(config.watch.len(), 7);
        assert_eq!(config.debounce_ms, 1_500);
        assert_eq!(config.push_interval_sec, 600);
        assert_eq!(config.storm_window_sec, 5);
        assert_eq!(config.mode, WatchMode::Watcher);
    }

    #[test]
    fn test_from_toml_str_parses_explicit_values() {
        let toml_content = r#"
workspace = "/tmp/workspace"
watch = ["SOUL.md", "memory/*.md"]
debounce_ms = 300
push_interval_sec = 120
storm_window_sec = 8
mode = "hook"
"#;

        let config = match WorkspaceConfig::from_toml_str(toml_content) {
            Ok(config) => config,
            Err(err) => panic!("failed to parse config from string: {err}"),
        };

        assert_eq!(config.workspace, PathBuf::from("/tmp/workspace"));
        assert_eq!(config.watch.len(), 2);
        assert_eq!(config.debounce_ms, 300);
        assert_eq!(config.push_interval_sec, 120);
        assert_eq!(config.storm_window_sec, 8);
        assert_eq!(config.mode, WatchMode::Hook);
    }

    #[test]
    fn test_missing_optional_fields_use_defaults() {
        let toml_content = r#"
workspace = "/tmp/minimal"
"#;

        let config = match WorkspaceConfig::from_toml_str(toml_content) {
            Ok(config) => config,
            Err(err) => panic!("failed to parse minimal config: {err}"),
        };

        assert_eq!(config.workspace, PathBuf::from("/tmp/minimal"));
        assert_eq!(config.watch.len(), 7);
        assert_eq!(config.debounce_ms, 1_500);
        assert_eq!(config.push_interval_sec, 600);
        assert_eq!(config.storm_window_sec, 5);
        assert_eq!(config.mode, WatchMode::Watcher);
    }
}
