use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::config::Config;
use crate::daemon::broker::{Broker, BrokerOptions};
use crate::db::models::{FeedbackEntry, JournalEntry};
use crate::db::store::Store;
use crate::plugin::{SuggestionCandidate, SuggestionRequest};
use crate::protocol::{self, ClientMessage, CycleDirection, DaemonMessage};

#[derive(Debug, Clone)]
struct SessionSuggestionState {
    request_id: u64,
    selected_index: usize,
    candidates: Vec<SuggestionCandidate>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
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

pub async fn run(
    socket_path: &Path,
    store: Arc<Mutex<Store>>,
    options: BrokerOptions,
) -> Result<()> {
    let _ = std::fs::remove_file(socket_path);

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    let broker = Arc::new(Broker::new(Arc::clone(&store), options));
    let session_state = Arc::new(Mutex::new(HashMap::<String, SessionSuggestionState>::new()));

    tracing::info!("daemon listening on {}", socket_path.display());

    let pid_path = socket_path.with_extension("pid");
    std::fs::write(&pid_path, std::process::id().to_string())?;

    loop {
        let (stream, _) = listener.accept().await?;
        let broker = Arc::clone(&broker);
        let store = Arc::clone(&store);
        let session_state = Arc::clone(&session_state);

        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let response = match protocol::parse_client_message(&line) {
                    Ok(ClientMessage::Suggest {
                        request_id,
                        buffer,
                        cursor,
                        cwd,
                        session_id,
                        last_command,
                    }) => {
                        let now = now_ms();

                        let req = SuggestionRequest {
                            request_id,
                            buffer,
                            cursor,
                            cwd: PathBuf::from(cwd),
                            session_id: session_id.clone(),
                            last_command,
                            timestamp_ms: now,
                        };

                        let candidates = broker.suggest(&req);
                        let response = suggestion_message(request_id, 0, &candidates);
                        session_state.lock().unwrap().insert(
                            session_id,
                            SessionSuggestionState {
                                request_id,
                                selected_index: 0,
                                candidates,
                            },
                        );
                        response
                    }
                    Ok(ClientMessage::Cycle {
                        session_id,
                        direction,
                    }) => {
                        let mut sessions = session_state.lock().unwrap();
                        if let Some(state) = sessions.get_mut(&session_id) {
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
                            suggestion_message(
                                state.request_id,
                                state.selected_index,
                                &state.candidates,
                            )
                        } else {
                            suggestion_message(0, 0, &[])
                        }
                    }
                    Ok(ClientMessage::Feedback {
                        command,
                        source,
                        score,
                        accepted,
                        session_id,
                    }) => {
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

                            if let Err(err) = store.lock().unwrap().insert_feedback(&entry) {
                                tracing::error!("failed to record feedback entry: {err}");
                            }
                        }

                        DaemonMessage::Ack { request_id: 0 }
                    }
                    Ok(ClientMessage::Record {
                        command,
                        cwd,
                        exit_code,
                        duration_ms,
                        session_id,
                    }) => {
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

                            if let Err(e) = store.lock().unwrap().insert_journal(&entry) {
                                tracing::error!("failed to record journal entry: {e}");
                            }
                        }

                        DaemonMessage::Ack { request_id: 0 }
                    }
                    Err(err) => DaemonMessage::Error {
                        message: format!("parse error: {err}"),
                        request_id: 0,
                    },
                };

                let mut line = protocol::encode_daemon_message(&response);
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}

pub fn default_socket_path() -> PathBuf {
    Config::default().socket_path()
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
