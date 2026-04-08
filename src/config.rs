use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_PATH_MAX_ENTRIES: usize = 256;
const DEFAULT_SHOW_HIDDEN: bool = false;
const DEFAULT_CD_FALLBACK_MODE: &str = "current_dir_only";
const DEFAULT_MAX_CANDIDATES: usize = 5;
const DEFAULT_SEED_FROM_HISTFILE: bool = true;
const DEFAULT_HISTFILE_PATH: &str = "";
const DEFAULT_HISTORY_SEED_MAX_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub path: PathConfig,
    pub cd: CdConfig,
    pub history: HistoryConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PathConfig {
    pub max_entries: usize,
    pub show_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CdConfig {
    pub fallback_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HistoryConfig {
    pub seed_from_histfile: bool,
    pub histfile_path: String,
    pub seed_max_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct UiConfig {
    pub max_candidates: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdFallbackMode {
    CurrentDirOnly,
    Disabled,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: PathConfig::default(),
            cd: CdConfig::default(),
            history: HistoryConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_PATH_MAX_ENTRIES,
            show_hidden: DEFAULT_SHOW_HIDDEN,
        }
    }
}

impl Default for CdConfig {
    fn default() -> Self {
        Self {
            fallback_mode: DEFAULT_CD_FALLBACK_MODE.into(),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            seed_from_histfile: DEFAULT_SEED_FROM_HISTFILE,
            histfile_path: DEFAULT_HISTFILE_PATH.into(),
            seed_max_entries: DEFAULT_HISTORY_SEED_MAX_ENTRIES,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            max_candidates: DEFAULT_MAX_CANDIDATES,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        match Self::default_path() {
            Some(path) => Self::load_from_path(&path),
            None => Ok(Self::default()),
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;
        toml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))
            .map(|config| config.normalized())
    }

    pub fn default_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(xdg).join("shellsuggest/config.toml"));
        }

        std::env::var("HOME")
            .ok()
            .map(|home| PathBuf::from(home).join(".config/shellsuggest/config.toml"))
    }

    pub fn cd_fallback_mode(&self) -> CdFallbackMode {
        match self.cd.fallback_mode.as_str() {
            "disabled" => CdFallbackMode::Disabled,
            _ => CdFallbackMode::CurrentDirOnly,
        }
    }

    pub fn history_seed_path(&self) -> Option<PathBuf> {
        if !self.history.seed_from_histfile {
            return None;
        }

        let configured = self.history.histfile_path.trim();
        if !configured.is_empty() {
            return Some(expand_tilde(configured));
        }

        std::env::var("HISTFILE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| expand_tilde(value.trim()))
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join(".zsh_history"))
            })
    }

    fn normalized(mut self) -> Self {
        if self.path.max_entries == 0 {
            self.path.max_entries = DEFAULT_PATH_MAX_ENTRIES;
        }
        if self.ui.max_candidates == 0 {
            self.ui.max_candidates = DEFAULT_MAX_CANDIDATES;
        }
        if self.history.seed_max_entries == 0 {
            self.history.seed_max_entries = DEFAULT_HISTORY_SEED_MAX_ENTRIES;
        }
        if self.cd.fallback_mode.trim().is_empty() {
            self.cd.fallback_mode = DEFAULT_CD_FALLBACK_MODE.into();
        }
        self
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "config:")?;
        writeln!(f, "  path.max_entries = {}", self.path.max_entries)?;
        writeln!(f, "  path.show_hidden = {}", self.path.show_hidden)?;
        writeln!(f, "  cd.fallback_mode = {}", self.cd.fallback_mode)?;
        writeln!(
            f,
            "  history.seed_from_histfile = {}",
            self.history.seed_from_histfile
        )?;
        writeln!(
            f,
            "  history.histfile_path = {}",
            self.history_seed_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "disabled".into())
        )?;
        writeln!(
            f,
            "  history.seed_max_entries = {}",
            self.history.seed_max_entries
        )?;
        write!(f, "  ui.max_candidates = {}", self.ui.max_candidates)
    }
}

fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(value));
    }

    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }

    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_defaults_when_config_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.toml");

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.path.max_entries, DEFAULT_PATH_MAX_ENTRIES);
        assert_eq!(config.path.show_hidden, DEFAULT_SHOW_HIDDEN);
        assert_eq!(config.ui.max_candidates, DEFAULT_MAX_CANDIDATES);
        assert_eq!(config.cd_fallback_mode(), CdFallbackMode::CurrentDirOnly);
        assert!(config.history.seed_from_histfile);
        assert_eq!(
            config.history.seed_max_entries,
            DEFAULT_HISTORY_SEED_MAX_ENTRIES
        );
    }

    #[test]
    fn test_override_values_from_config_file() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[path]
max_entries = 128
show_hidden = true

[cd]
fallback_mode = "disabled"

[history]
seed_from_histfile = false
histfile_path = "~/alt-history"
seed_max_entries = 123

[ui]
max_candidates = 7
"#,
        );

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.path.max_entries, 128);
        assert!(config.path.show_hidden);
        assert_eq!(config.cd_fallback_mode(), CdFallbackMode::Disabled);
        assert!(!config.history.seed_from_histfile);
        assert_eq!(config.history.seed_max_entries, 123);
        assert_eq!(config.history.histfile_path, "~/alt-history");
        assert_eq!(config.ui.max_candidates, 7);
    }

    #[test]
    fn test_invalid_toml_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "not = [valid");

        let error = Config::load_from_path(&path).unwrap_err().to_string();

        assert!(error.contains("failed to parse config"));
        assert!(error.contains("config.toml"));
    }

    #[test]
    fn test_zero_values_fall_back_to_defaults() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[path]
max_entries = 0

[history]
seed_max_entries = 0

[ui]
max_candidates = 0
"#,
        );

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.path.max_entries, DEFAULT_PATH_MAX_ENTRIES);
        assert_eq!(
            config.history.seed_max_entries,
            DEFAULT_HISTORY_SEED_MAX_ENTRIES
        );
        assert_eq!(config.ui.max_candidates, DEFAULT_MAX_CANDIDATES);
    }

    #[test]
    fn test_history_seed_path_prefers_configured_value() {
        let config = Config {
            history: HistoryConfig {
                seed_from_histfile: true,
                histfile_path: "~/custom_hist".into(),
                seed_max_entries: DEFAULT_HISTORY_SEED_MAX_ENTRIES,
            },
            ..Config::default()
        };

        let path = config.history_seed_path().unwrap();
        assert!(path.ends_with("custom_hist"));
    }
}
