use smallvec::SmallVec;
use std::sync::{Arc, Mutex};

use crate::db::store::Store;
use crate::plugin::{InlineProvider, SuggestionCandidate, SuggestionKind, SuggestionRequest};
use crate::ranking;

pub struct HistoryPlugin {
    store: Arc<Mutex<Store>>,
}

impl HistoryPlugin {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store }
    }
}

impl InlineProvider for HistoryPlugin {
    fn name(&self) -> &'static str {
        "history"
    }

    fn supports(&self, req: &SuggestionRequest) -> bool {
        !req.buffer.is_empty() && ranking::extract_command(&req.buffer) != Some("cd")
    }

    fn suggest(&self, req: &SuggestionRequest) -> SmallVec<[SuggestionCandidate; 4]> {
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return SmallVec::new(),
        };

        let (entries, source, success_bonus) =
            match store.ranked_commands_by_prefix(&req.buffer, 20) {
                Ok(entries) if !entries.is_empty() => (entries, "history", 1.0),
                Ok(_) => match store.seeded_commands_by_prefix(&req.buffer, 20) {
                    Ok(entries) => (entries, "history_seed", 0.0),
                    Err(_) => return SmallVec::new(),
                },
                Err(_) => return SmallVec::new(),
            };

        let mut candidates = SmallVec::new();

        for entry in entries {
            let prefix_score = ranking::prefix_exactness(&entry.command_line, &req.buffer);
            let recency_score = ranking::recency(entry.timestamp, req.timestamp_ms);
            let freq_score = ranking::frequency(entry.frequency);

            let features = ranking::RankingFeatures {
                prefix_exactness: prefix_score,
                cwd_similarity: 0.0,
                path_exists: 0.0,
                recency: recency_score,
                frequency: freq_score,
                last_command_transition: 0.0,
                success_bonus,
            };

            candidates.push(SuggestionCandidate {
                text: entry.command_line,
                source: source.into(),
                score: features.score(),
                kind: SuggestionKind::Command,
            });
        }

        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::JournalEntry;
    use std::path::PathBuf;

    fn setup() -> (Arc<Mutex<Store>>, SuggestionRequest) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let req = SuggestionRequest {
            request_id: 1,
            buffer: String::new(),
            cursor: 0,
            cwd: PathBuf::from("/test"),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        };
        (store, req)
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
    fn test_empty_buffer_not_supported() {
        let (store, req) = setup();
        let plugin = HistoryPlugin::new(store);
        assert!(!plugin.supports(&req));
    }

    #[test]
    fn test_cd_uses_cwd_history_instead_of_global_history() {
        let (store, mut req) = setup();
        insert(&store, "cd src", "/a", 1000);

        req.buffer = "cd ".into();
        req.cursor = 3;

        let plugin = HistoryPlugin::new(store);
        assert!(!plugin.supports(&req));
    }

    #[test]
    fn test_basic_prefix_match() {
        let (store, mut req) = setup();
        insert(&store, "cd src", "/a", 1000);
        insert(&store, "cd scripts", "/b", 2000);
        insert(&store, "vim main.rs", "/a", 3000);

        req.buffer = "cd ".into();
        req.cursor = 3;

        let plugin = HistoryPlugin::new(store);
        let results = plugin.suggest(&req);

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|c| c.text.starts_with("cd ")));
        assert!(results.iter().all(|c| c.source == "history"));
    }

    #[test]
    fn test_deduplicates() {
        let (store, mut req) = setup();
        insert(&store, "ls", "/a", 1000);
        insert(&store, "ls", "/b", 2000);
        insert(&store, "ls", "/a", 3000);

        req.buffer = "ls".into();
        req.cursor = 2;

        let plugin = HistoryPlugin::new(store);
        let results = plugin.suggest(&req);

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_no_results_for_unknown_prefix() {
        let (store, mut req) = setup();
        insert(&store, "cd src", "/a", 1000);

        req.buffer = "zzz".into();
        req.cursor = 3;

        let plugin = HistoryPlugin::new(store);
        let results = plugin.suggest(&req);

        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_falls_back_to_seeded_history_when_live_history_missing() {
        let (store, mut req) = setup();
        store
            .lock()
            .unwrap()
            .replace_seeded_command_stats(&[crate::db::models::SeededCommandStat {
                command_line: "git status".into(),
                latest_timestamp: 5_000,
                sample_count: 3,
            }])
            .unwrap();

        req.buffer = "git ".into();
        req.cursor = 4;

        let plugin = HistoryPlugin::new(store);
        let results = plugin.suggest(&req);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "git status");
        assert_eq!(results[0].source, "history_seed");
    }

    #[test]
    fn test_live_history_beats_seeded_history_for_same_prefix() {
        let (store, mut req) = setup();
        insert(&store, "git commit", "/a", 4_000);
        store
            .lock()
            .unwrap()
            .replace_seeded_command_stats(&[crate::db::models::SeededCommandStat {
                command_line: "git status".into(),
                latest_timestamp: 5_000,
                sample_count: 9,
            }])
            .unwrap();

        req.buffer = "git ".into();
        req.cursor = 4;

        let plugin = HistoryPlugin::new(store);
        let results = plugin.suggest(&req);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "git commit");
        assert_eq!(results[0].source, "history");
    }
}
