use smallvec::SmallVec;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::CdFallbackMode;
use crate::db::store::Store;
use crate::plugin::{InlineProvider, SuggestionCandidate, SuggestionKind, SuggestionRequest};
use crate::ranking;

pub struct CdAssistPlugin {
    store: Arc<Mutex<Store>>,
    mode: CdFallbackMode,
}

impl CdAssistPlugin {
    pub fn new(store: Arc<Mutex<Store>>, mode: CdFallbackMode) -> Self {
        Self { store, mode }
    }

    fn extract_cd_token<'a>(&self, req: &'a SuggestionRequest) -> Option<&'a str> {
        if self.mode == CdFallbackMode::Disabled {
            return None;
        }

        if ranking::extract_command(&req.buffer) != Some("cd") || req.buffer.ends_with(' ') {
            return None;
        }

        let mut parts = req.buffer.split_whitespace();
        parts.next()?;
        parts.next()
    }

    fn resolve_dir_and_prefix<'a>(token: &'a str, cwd: &Path) -> Option<(PathBuf, &'a str)> {
        if token.is_empty() || token.starts_with('/') || token.starts_with('~') {
            return None;
        }

        if let Some(slash_pos) = token.rfind('/') {
            let dir_part = &token[..=slash_pos];
            let prefix = &token[slash_pos + 1..];
            if prefix.is_empty() {
                return None;
            }
            Some((cwd.join(dir_part), prefix))
        } else {
            Some((cwd.to_path_buf(), token))
        }
    }
}

impl InlineProvider for CdAssistPlugin {
    fn name(&self) -> &'static str {
        "cd_assist"
    }

    fn supports(&self, req: &SuggestionRequest) -> bool {
        self.extract_cd_token(req).is_some()
    }

    fn suggest(&self, req: &SuggestionRequest) -> SmallVec<[SuggestionCandidate; 4]> {
        let Some(token) = self.extract_cd_token(req) else {
            return SmallVec::new();
        };

        let cwd_string = req.cwd.to_string_lossy().to_string();
        let store = match self.store.lock() {
            Ok(store) => store,
            Err(_) => return SmallVec::new(),
        };
        if store
            .has_ranked_command_prefix_and_cwd(&req.buffer, &cwd_string)
            .unwrap_or(false)
        {
            return SmallVec::new();
        }
        drop(store);

        let (dir, prefix) = match Self::resolve_dir_and_prefix(token, &req.cwd) {
            Some(result) => result,
            None => return SmallVec::new(),
        };

        let mut entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return SmallVec::new(),
        };

        let mut candidates = SmallVec::new();
        let prefix_lower = prefix.to_lowercase();
        while let Some(Ok(entry)) = entries.next() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) && !name.to_lowercase().starts_with(&prefix_lower) {
                continue;
            }

            let is_dir = match entry.file_type() {
                Ok(file_type) => file_type.is_dir(),
                Err(_) => false,
            };
            if !is_dir {
                continue;
            }

            let path_part = if token.contains('/') {
                let dir_prefix = &token[..token.rfind('/').unwrap() + 1];
                format!("{dir_prefix}{name}")
            } else {
                name.clone()
            };

            let features = ranking::RankingFeatures {
                prefix_exactness: ranking::prefix_exactness(&name, prefix),
                cwd_similarity: 1.0,
                path_exists: 1.0,
                recency: 0.0,
                frequency: 0.0,
                last_command_transition: 0.0,
                success_bonus: 0.0,
            };

            candidates.push(SuggestionCandidate {
                text: format!("cd {path_part}"),
                source: "cd_assist".into(),
                score: features.score(),
                kind: SuggestionKind::Path,
            });
        }

        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::JournalEntry;
    use tempfile::TempDir;

    fn request(buffer: &str, cwd: &Path) -> SuggestionRequest {
        SuggestionRequest {
            request_id: 1,
            buffer: buffer.into(),
            cursor: buffer.len(),
            cwd: cwd.to_path_buf(),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        }
    }

    #[test]
    fn test_cd_assist_only_runs_for_cd_prefixes() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let plugin = CdAssistPlugin::new(store, CdFallbackMode::CurrentDirOnly);
        assert!(!plugin.supports(&request("vim s", Path::new("/tmp"))));
        assert!(!plugin.supports(&request("cd ", Path::new("/tmp"))));
        assert!(plugin.supports(&request("cd s", Path::new("/tmp"))));
    }

    #[test]
    fn test_cd_assist_uses_local_directories_when_no_history_exists() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("scripts")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let plugin = CdAssistPlugin::new(store, CdFallbackMode::CurrentDirOnly);
        let results = plugin.suggest(&request("cd s", tmp.path()));

        assert!(results.iter().any(|candidate| candidate.text == "cd src"));
        assert!(results
            .iter()
            .any(|candidate| candidate.text == "cd scripts"));
    }

    #[test]
    fn test_cd_assist_is_disabled_when_cwd_history_exists() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        store
            .lock()
            .unwrap()
            .insert_journal(&JournalEntry {
                id: None,
                command_line: "cd src".into(),
                cwd: tmp.path().to_string_lossy().to_string(),
                exit_code: Some(0),
                duration_ms: Some(10),
                session_id: "test".into(),
                timestamp: 1_000,
            })
            .unwrap();

        let plugin = CdAssistPlugin::new(store, CdFallbackMode::CurrentDirOnly);
        let results = plugin.suggest(&request("cd s", tmp.path()));

        assert!(results.is_empty());
    }

    #[test]
    fn test_cd_assist_supports_relative_path_prefixes() {
        let tmp = TempDir::new().unwrap();
        let app_dir = tmp.path().join("app");
        std::fs::create_dir(&app_dir).unwrap();
        std::fs::create_dir(app_dir.join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("shared")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let plugin = CdAssistPlugin::new(store, CdFallbackMode::CurrentDirOnly);

        let current_results = plugin.suggest(&request("cd ./s", &app_dir));
        assert!(current_results
            .iter()
            .any(|candidate| candidate.text == "cd ./src"));

        let parent_results = plugin.suggest(&request("cd ../s", &app_dir));
        assert!(parent_results
            .iter()
            .any(|candidate| candidate.text == "cd ../shared"));
    }

    #[test]
    fn test_cd_assist_can_be_disabled_via_config_mode() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let plugin = CdAssistPlugin::new(store, CdFallbackMode::Disabled);
        let req = request("cd s", tmp.path());

        assert!(!plugin.supports(&req));
        assert!(plugin.suggest(&req).is_empty());
    }
}
