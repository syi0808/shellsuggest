use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::config::Config;
use crate::daemon::broker::{Broker, BrokerOptions};
use crate::db::models::{FeedbackEntry, JournalEntry};
use crate::db::store::Store;
use crate::history_seed;
use crate::plugin::{SuggestionCandidate, SuggestionRequest};
use crate::protocol::{ClientMessage, CycleDirection, DaemonMessage};

#[derive(Debug, Clone)]
struct SessionSuggestionState {
    request_id: u64,
    selected_index: usize,
    candidates: Vec<SuggestionCandidate>,
}

pub struct QueryRuntime {
    broker: Broker,
    store: Arc<Mutex<Store>>,
    session_state: HashMap<String, SessionSuggestionState>,
}

impl QueryRuntime {
    pub fn from_config(config: &Config) -> Result<Self> {
        let db_path = default_db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let store = Arc::new(Mutex::new(Store::open(db_path.to_str().unwrap())?));
        {
            let store_guard = store.lock().unwrap();
            match history_seed::prime_store_from_histfile(&store_guard, config) {
                Ok(summary) => {
                    if let Some(path) = summary.source_path {
                        tracing::info!(
                            path = %path.display(),
                            parsed_entries = summary.parsed_entries,
                            imported_commands = summary.imported_commands,
                            "seeded history fallback from HISTFILE"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to seed history fallback from HISTFILE");
                }
            }
        }

        Ok(Self::new(
            store,
            BrokerOptions {
                path_show_hidden: config.path.show_hidden,
                path_max_entries: config.path.max_entries,
                max_candidates: config.ui.max_candidates,
                cd_fallback_mode: config.cd_fallback_mode(),
            },
        ))
    }

    pub fn new(store: Arc<Mutex<Store>>, options: BrokerOptions) -> Self {
        Self {
            broker: Broker::new(Arc::clone(&store), options),
            store,
            session_state: HashMap::new(),
        }
    }

    pub fn handle_message(&mut self, message: ClientMessage) -> DaemonMessage {
        match message {
            ClientMessage::Suggest {
                request_id,
                buffer,
                cursor,
                cwd,
                session_id,
                last_command,
            } => {
                let req = SuggestionRequest {
                    request_id,
                    buffer,
                    cursor,
                    cwd: PathBuf::from(cwd),
                    session_id: session_id.clone(),
                    last_command,
                    timestamp_ms: now_ms(),
                };

                let candidates = self.broker.suggest(&req);
                let response = suggestion_message(request_id, 0, &candidates);
                self.session_state.insert(
                    session_id,
                    SessionSuggestionState {
                        request_id,
                        selected_index: 0,
                        candidates,
                    },
                );
                response
            }
            ClientMessage::Cycle {
                session_id,
                direction,
            } => {
                if let Some(state) = self.session_state.get_mut(&session_id) {
                    if !state.candidates.is_empty() {
                        state.selected_index = match direction {
                            CycleDirection::Next => {
                                (state.selected_index + 1) % state.candidates.len()
                            }
                            CycleDirection::Prev => {
                                (state.selected_index + state.candidates.len() - 1)
                                    % state.candidates.len()
                            }
                        };
                    }
                    suggestion_message(state.request_id, state.selected_index, &state.candidates)
                } else {
                    suggestion_message(0, 0, &[])
                }
            }
            ClientMessage::Feedback {
                command,
                source,
                score,
                accepted,
                session_id,
            } => {
                if !command.is_empty() && !source.is_empty() {
                    let entry = FeedbackEntry {
                        id: None,
                        command_line: command,
                        source,
                        score: Some(score),
                        accepted,
                        session_id,
                        timestamp: now_ms(),
                    };

                    if let Err(err) = self.store.lock().unwrap().insert_feedback(&entry) {
                        tracing::error!("failed to record feedback entry: {err}");
                    }
                }

                DaemonMessage::Ack { request_id: 0 }
            }
            ClientMessage::Record {
                command,
                cwd,
                exit_code,
                duration_ms,
                session_id,
            } => {
                if exit_code == 0 {
                    let entry = JournalEntry {
                        id: None,
                        command_line: command,
                        cwd,
                        exit_code: Some(exit_code),
                        duration_ms: Some(duration_ms),
                        session_id,
                        timestamp: now_ms(),
                    };

                    if let Err(err) = self.store.lock().unwrap().insert_journal(&entry) {
                        tracing::error!("failed to record journal entry: {err}");
                    }
                }

                DaemonMessage::Ack { request_id: 0 }
            }
        }
    }
}

pub fn default_db_path() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share")
        });
    data_dir.join("shellsuggest/journal.db")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn suggestion_message(
    request_id: u64,
    selected_index: usize,
    candidates: &[SuggestionCandidate],
) -> DaemonMessage {
    match candidates.get(selected_index) {
        Some(candidate) => DaemonMessage::Suggestion {
            request_id,
            candidate_count: candidates.len(),
            candidate_index: selected_index,
            source: candidate.source.clone(),
            score: candidate.score,
            text: candidate.text.clone(),
        },
        None => DaemonMessage::Suggestion {
            request_id,
            candidate_count: 0,
            candidate_index: 0,
            source: String::new(),
            score: 0.0,
            text: String::new(),
        },
    }
}
