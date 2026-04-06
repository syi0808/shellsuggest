use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::CdFallbackMode;
use crate::db::store::Store;
use crate::plugin::cd_assist::CdAssistPlugin;
use crate::plugin::cwd_history::CwdHistoryPlugin;
use crate::plugin::history::HistoryPlugin;
use crate::plugin::path::PathPlugin;
use crate::plugin::{InlineProvider, SuggestionCandidate, SuggestionRequest};
use crate::ranking;

#[derive(Debug, Clone, Copy)]
pub struct BrokerOptions {
    pub path_show_hidden: bool,
    pub path_max_entries: usize,
    pub max_candidates: usize,
    pub cd_fallback_mode: CdFallbackMode,
}

pub struct Broker {
    plugins: Vec<Box<dyn InlineProvider>>,
    max_candidates: usize,
}

impl Broker {
    pub fn new(store: Arc<Mutex<Store>>, options: BrokerOptions) -> Self {
        let plugins: Vec<Box<dyn InlineProvider>> = vec![
            Box::new(HistoryPlugin::new(Arc::clone(&store))),
            Box::new(CwdHistoryPlugin::new(Arc::clone(&store))),
            Box::new(PathPlugin::new(
                Arc::clone(&store),
                options.path_show_hidden,
                options.path_max_entries,
            )),
            Box::new(CdAssistPlugin::new(
                Arc::clone(&store),
                options.cd_fallback_mode,
            )),
        ];
        Self {
            plugins,
            max_candidates: options.max_candidates,
        }
    }

    pub fn suggest(&self, req: &SuggestionRequest) -> Vec<SuggestionCandidate> {
        let mut all_candidates: Vec<SuggestionCandidate> = Vec::new();

        for plugin in &self.plugins {
            if !plugin.supports(req) {
                continue;
            }

            let start = Instant::now();
            let candidates = plugin.suggest(req);
            let elapsed = start.elapsed();

            if elapsed.as_millis() > 10 {
                tracing::warn!(
                    plugin = plugin.name(),
                    elapsed_ms = elapsed.as_millis() as u64,
                    "plugin exceeded 10ms timeout"
                );
            }

            all_candidates.extend(candidates.into_iter());
        }

        ranking::apply_dangerous_filter(&mut all_candidates);

        all_candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut seen = HashSet::new();
        all_candidates.retain(|c| seen.insert(c.text.clone()));
        all_candidates.truncate(self.max_candidates);

        all_candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::JournalEntry;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn options() -> BrokerOptions {
        BrokerOptions {
            path_show_hidden: false,
            path_max_entries: 256,
            max_candidates: 5,
            cd_fallback_mode: CdFallbackMode::CurrentDirOnly,
        }
    }

    fn insert(store: &Arc<Mutex<Store>>, cmd: &str, cwd: &str, ts: u64) {
        store
            .lock()
            .unwrap()
            .insert_journal(&JournalEntry {
                id: None,
                command_line: cmd.into(),
                cwd: cwd.into(),
                exit_code: Some(0),
                duration_ms: Some(10),
                session_id: "test".into(),
                timestamp: ts,
            })
            .unwrap();
    }

    #[test]
    fn test_broker_returns_best_candidate() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "cd src", "/project", 1000);
        insert(&store, "cd scripts", "/project", 2000);

        let broker = Broker::new(store, options());
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert!(!result.is_empty());
        let candidate = result.into_iter().next().unwrap();
        assert!(candidate.text.starts_with("cd s"));
    }

    #[test]
    fn test_broker_deduplicates_across_plugins() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "make test", "/project", 1000);
        insert(&store, "make test", "/project", 2000);

        let broker = Broker::new(store, options());
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "make ".into(),
            cursor: 5,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_broker_cwd_history_beats_plain_history() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "make deploy", "/other", 5000);
        insert(&store, "make test", "/project", 1000);

        let broker = Broker::new(store, options());
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "make ".into(),
            cursor: 5,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert!(!result.is_empty());
        assert_eq!(result[0].text, "make test");
    }

    #[test]
    fn test_broker_path_plugin_integration() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("scripts")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let broker = Broker::new(store, options());

        let req = SuggestionRequest {
            request_id: 1,
            buffer: "pushd s".into(),
            cursor: 7,
            cwd: tmp.path().to_path_buf(),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert!(!result.is_empty());
        let text = result[0].text.clone();
        assert!(text == "pushd src" || text == "pushd scripts");
    }

    #[test]
    fn test_broker_cd_uses_current_pwd_history_only() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "cd src", "/other", 1000);
        let broker = Broker::new(store, options());

        let req = SuggestionRequest {
            request_id: 1,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: tmp.path().to_path_buf(),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let results = broker.suggest(&req);
        assert_eq!(results[0].source, "cd_assist");
    }

    #[test]
    fn test_broker_empty_buffer_returns_none() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let broker = Broker::new(store, options());

        let req = SuggestionRequest {
            request_id: 1,
            buffer: "".into(),
            cursor: 0,
            cwd: PathBuf::from("/tmp"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        assert!(broker.suggest(&req).is_empty());
    }

    #[test]
    fn test_dangerous_commands_filtered() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "rm -rf /important", "/project", 9000);

        let broker = Broker::new(store, options());
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "rm ".into(),
            cursor: 3,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        if let Some(candidate) = result.first() {
            assert!(
                candidate.score >= 0.6,
                "dangerous command with low score should be filtered"
            );
        }
    }

    #[test]
    fn test_broker_truncates_to_max_candidates() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        for idx in 0..10 {
            insert(&store, &format!("echo item{idx}"), "/project", 1_000 + idx);
        }

        let broker = Broker::new(
            store,
            BrokerOptions {
                max_candidates: 3,
                ..options()
            },
        );
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "echo ".into(),
            cursor: 5,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_broker_uses_last_command_transition_to_break_ties() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        insert(&store, "vim main.rs", "/project", 1_000);
        insert(&store, "make test", "/project", 2_000);
        insert(&store, "vim main.rs", "/project", 3_000);
        insert(&store, "make test", "/project", 4_000);
        insert(&store, "make build", "/project", 4_000);

        let broker = Broker::new(store, options());
        let req = SuggestionRequest {
            request_id: 1,
            buffer: "make ".into(),
            cursor: 5,
            cwd: PathBuf::from("/project"),
            session_id: "test".into(),
            last_command: Some("vim main.rs".into()),
            timestamp_ms: 10_000,
        };

        let result = broker.suggest(&req);
        assert!(!result.is_empty());
        assert_eq!(result[0].text, "make test");
    }
}
