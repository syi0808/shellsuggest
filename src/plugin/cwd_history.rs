use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::db::store::Store;
use crate::plugin::{InlineProvider, SuggestionCandidate, SuggestionKind, SuggestionRequest};
use crate::ranking;

pub struct CwdHistoryPlugin {
    store: Arc<Mutex<Store>>,
}

impl CwdHistoryPlugin {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store }
    }
}

impl InlineProvider for CwdHistoryPlugin {
    fn name(&self) -> &'static str {
        "cwd_history"
    }

    fn supports(&self, req: &SuggestionRequest) -> bool {
        !req.buffer.is_empty()
    }

    fn suggest(&self, req: &SuggestionRequest) -> SmallVec<[SuggestionCandidate; 4]> {
        let store = self.store.lock().unwrap();
        let cwd = req.cwd.to_string_lossy();
        let is_cd = ranking::extract_command(&req.buffer) == Some("cd");

        // Query exact cwd matches
        let cwd_entries = store
            .ranked_commands_by_prefix_and_cwd(&req.buffer, &cwd, 20)
            .unwrap_or_default();

        // For `cd`, only suggest entries executed from the current PWD.
        let parent_entries = if is_cd {
            Vec::new()
        } else {
            let parent_cwd = req.cwd.parent().map(|p| p.to_string_lossy().to_string());
            if let Some(ref parent) = parent_cwd {
                store
                    .ranked_commands_by_prefix_and_cwd(&req.buffer, parent, 10)
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        };

        let mut seen = HashSet::new();
        let mut ranked_entries = Vec::with_capacity(cwd_entries.len() + parent_entries.len());
        for entry in cwd_entries {
            if seen.insert(entry.command_line.clone()) {
                ranked_entries.push((entry, 1.0));
            }
        }
        for entry in parent_entries {
            if ranking::extract_command(&entry.command_line) == Some("cd") {
                continue;
            }
            if seen.insert(entry.command_line.clone()) {
                ranked_entries.push((entry, 0.5));
            }
        }

        let mut transition_cache = HashMap::new();

        let mut candidates = SmallVec::new();
        for (entry, cwd_similarity) in ranked_entries {
            let transition_score = self.transition_score_locked(
                &store,
                req,
                &entry.command_line,
                &mut transition_cache,
            );

            let features = ranking::RankingFeatures {
                prefix_exactness: ranking::prefix_exactness(&entry.command_line, &req.buffer),
                cwd_similarity,
                path_exists: ranking::path_exists_score(&entry.command_line, &req.cwd),
                recency: ranking::recency(entry.timestamp, req.timestamp_ms),
                frequency: ranking::frequency(entry.frequency),
                last_command_transition: transition_score,
                success_bonus: 1.0,
            };

            candidates.push(SuggestionCandidate {
                text: entry.command_line,
                source: "cwd_history".into(),
                score: features.score(),
                kind: SuggestionKind::Command,
            });
        }

        candidates
    }
}

impl CwdHistoryPlugin {
    fn transition_score_locked(
        &self,
        store: &Store,
        req: &SuggestionRequest,
        candidate_cmd: &str,
        transition_cache: &mut HashMap<String, Vec<(String, u64)>>,
    ) -> f32 {
        let prev = match &req.last_command {
            Some(cmd) => cmd,
            None => return 0.0,
        };
        let prefix = ranking::extract_command(candidate_cmd)
            .unwrap_or("")
            .to_string();
        let transitions = transition_cache
            .entry(prefix.clone())
            .or_insert_with(|| store.transition_count(prev, &prefix, 5).unwrap_or_default());

        let total: u64 = transitions.iter().map(|(_, c)| c).sum();
        if total == 0 {
            return 0.0;
        }

        transitions
            .iter()
            .find(|(cmd, _)| cmd == candidate_cmd)
            .map(|(_, count)| *count as f32 / total as f32)
            .unwrap_or(0.0)
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
            cwd: PathBuf::from("/project"),
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
    fn test_cwd_exact_match_scores_higher() {
        let (store, mut req) = setup();
        insert(&store, "make test", "/project", 1000);
        insert(&store, "make build", "/other", 2000);

        req.buffer = "make ".into();
        req.cursor = 5;

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        assert!(results.iter().any(|c| c.text == "make test"));
        assert!(!results.iter().any(|c| c.text == "make build"));
    }

    #[test]
    fn test_parent_cwd_included() {
        let (store, mut req) = setup();
        req.cwd = PathBuf::from("/project/src");

        insert(&store, "make test", "/project", 1000);

        req.buffer = "make ".into();
        req.cursor = 5;

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        assert!(results.iter().any(|c| c.text == "make test"));
        let candidate = results.iter().find(|c| c.text == "make test").unwrap();
        assert!(candidate.score > 0.0);
    }

    #[test]
    fn test_cd_does_not_fall_back_to_parent_cwd() {
        let (store, mut req) = setup();
        req.cwd = PathBuf::from("/project/src");

        insert(&store, "cd nested", "/project", 1000);

        req.buffer = "cd n".into();
        req.cursor = 4;

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        assert!(results.is_empty());
    }

    #[test]
    fn test_partial_cd_prefix_does_not_pull_parent_cd_history() {
        let (store, mut req) = setup();
        req.cwd = PathBuf::from("/project/src");

        insert(&store, "cd nested", "/project", 1000);

        req.buffer = "c".into();
        req.cursor = 1;

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        assert!(!results.iter().any(|c| c.text == "cd nested"));
    }

    #[test]
    fn test_deduplicates_across_cwd_levels() {
        let (store, mut req) = setup();
        req.cwd = PathBuf::from("/project/src");

        insert(&store, "make test", "/project/src", 1000);
        insert(&store, "make test", "/project", 2000);

        req.buffer = "make ".into();
        req.cursor = 5;

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        let make_tests: Vec<_> = results.iter().filter(|c| c.text == "make test").collect();
        assert_eq!(make_tests.len(), 1);
    }

    #[test]
    fn test_transition_bonus() {
        let (store, mut req) = setup();
        insert(&store, "vim main.rs", "/project", 1000);
        insert(&store, "make test", "/project", 2000);
        insert(&store, "vim main.rs", "/project", 3000);
        insert(&store, "make test", "/project", 4000);

        req.buffer = "make ".into();
        req.cursor = 5;
        req.last_command = Some("vim main.rs".into());

        let plugin = CwdHistoryPlugin::new(Arc::clone(&store));
        let results = plugin.suggest(&req);

        assert!(results.iter().any(|c| c.text == "make test"));
    }
}
