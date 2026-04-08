use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

use shellsuggest::db::models::{FeedbackEntry, JournalEntry, PathCacheEntry};
use shellsuggest::db::store::Store;
use shellsuggest::protocol::{self, ClientMessage, CycleDirection, DaemonMessage};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

fn test_binary_path() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_shellsuggest") {
        return PathBuf::from(path);
    }

    let current = std::env::current_exe().unwrap();
    current
        .parent()
        .and_then(|path| path.parent())
        .unwrap()
        .join("shellsuggest")
}

fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("shellsuggest/journal.db")
}

fn open_store(data_root: &Path) -> Store {
    let db_path = db_path(data_root);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    Store::open(db_path.to_str().unwrap()).unwrap()
}

fn spawn_query(config_root: &Path, data_root: &Path) -> Child {
    Command::new(test_binary_path())
        .arg("query")
        .env("XDG_CONFIG_HOME", config_root)
        .env("XDG_DATA_HOME", data_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

async fn read_query_message(
    lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
) -> DaemonMessage {
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout waiting for line")
        .expect("io error")
        .expect("no line");
    protocol::parse_daemon_message(&line).unwrap()
}

async fn send_client_message(stdin: &mut ChildStdin, message: &ClientMessage) {
    let mut line = protocol::encode_client_message(message);
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn test_query_suggest_record_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let data_root = tmp.path().join("data");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(config_root.join("shellsuggest")).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::create_dir_all(workspace.join("scripts")).unwrap();

    let store = open_store(&data_root);
    store
        .insert_journal(&JournalEntry {
            id: None,
            command_line: "cd src".into(),
            cwd: workspace.to_string_lossy().to_string(),
            exit_code: Some(0),
            duration_ms: Some(5),
            session_id: "seed".into(),
            timestamp: 1_000,
        })
        .unwrap();

    let mut child = spawn_query(&config_root, &data_root);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    send_client_message(
        &mut stdin,
        &ClientMessage::Suggest {
            request_id: 1,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: workspace.to_string_lossy().to_string(),
            session_id: "integration-test".into(),
            last_command: None,
        },
    )
    .await;

    match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            request_id,
            source,
            candidate_index,
            candidate_count,
            ..
        } => {
            assert_eq!(request_id, 1);
            assert_eq!(source, "cwd_history");
            assert_eq!(candidate_index, 0);
            assert_eq!(candidate_count, 1);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    send_client_message(
        &mut stdin,
        &ClientMessage::Record {
            command: "cd src".into(),
            cwd: workspace.to_string_lossy().to_string(),
            exit_code: 0,
            duration_ms: 5,
            session_id: "integration-test".into(),
        },
    )
    .await;
    assert_eq!(
        read_query_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );

    send_client_message(
        &mut stdin,
        &ClientMessage::Record {
            command: "cd missing-dir".into(),
            cwd: workspace.to_string_lossy().to_string(),
            exit_code: 1,
            duration_ms: 5,
            session_id: "integration-test".into(),
        },
    )
    .await;
    assert_eq!(
        read_query_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );

    drop(stdin);
    let _ = child.kill().await;

    let store = open_store(&data_root);
    let successful = store.query_by_prefix("cd ", 10).unwrap();
    assert!(successful.iter().any(|entry| entry.command_line == "cd src"));
    assert!(!successful
        .iter()
        .any(|entry| entry.command_line == "cd missing-dir"));
}

#[tokio::test]
async fn test_query_cycle_and_feedback_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let data_root = tmp.path().join("data");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(config_root.join("shellsuggest")).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::create_dir_all(workspace.join("scripts")).unwrap();

    let mut child = spawn_query(&config_root, &data_root);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    send_client_message(
        &mut stdin,
        &ClientMessage::Suggest {
            request_id: 7,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: workspace.to_string_lossy().to_string(),
            session_id: "cycle-test".into(),
            last_command: None,
        },
    )
    .await;

    let (second_text, second_source, second_score) = match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            request_id,
            candidate_count,
            candidate_index,
            ..
        } => {
            assert_eq!(request_id, 7);
            assert_eq!(candidate_count, 2);
            assert_eq!(candidate_index, 0);

            send_client_message(
                &mut stdin,
                &ClientMessage::Cycle {
                    session_id: "cycle-test".into(),
                    direction: CycleDirection::Next,
                },
            )
            .await;

            match read_query_message(&mut lines).await {
                DaemonMessage::Suggestion {
                    request_id: second_request_id,
                    candidate_count: second_candidate_count,
                    candidate_index: second_candidate_index,
                    text,
                    source,
                    score,
                } => {
                    assert_eq!(second_request_id, request_id);
                    assert_eq!(second_candidate_count, 2);
                    assert_eq!(second_candidate_index, 1);
                    (text, source, score)
                }
                other => panic!("unexpected cycle response: {other:?}"),
            }
        }
        other => panic!("unexpected response: {other:?}"),
    };

    send_client_message(
        &mut stdin,
        &ClientMessage::Feedback {
            command: second_text,
            source: second_source,
            score: second_score,
            accepted: true,
            session_id: "cycle-test".into(),
        },
    )
    .await;

    assert_eq!(
        read_query_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );

    drop(stdin);
    let _ = child.kill().await;

    let store = open_store(&data_root);
    assert_eq!(store.feedback_counts().unwrap(), (1, 0));
}

#[tokio::test]
async fn test_query_cycle_wraps_around_in_both_directions() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let data_root = tmp.path().join("data");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(config_root.join("shellsuggest")).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::create_dir_all(workspace.join("scripts")).unwrap();

    let mut child = spawn_query(&config_root, &data_root);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    send_client_message(
        &mut stdin,
        &ClientMessage::Suggest {
            request_id: 11,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: workspace.to_string_lossy().to_string(),
            session_id: "cycle-wrap-test".into(),
            last_command: None,
        },
    )
    .await;

    let first_text = match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            request_id,
            candidate_count,
            candidate_index,
            text,
            ..
        } => {
            assert_eq!(request_id, 11);
            assert_eq!(candidate_count, 2);
            assert_eq!(candidate_index, 0);
            text
        }
        other => panic!("unexpected response: {other:?}"),
    };

    send_client_message(
        &mut stdin,
        &ClientMessage::Cycle {
            session_id: "cycle-wrap-test".into(),
            direction: CycleDirection::Prev,
        },
    )
    .await;

    let wrapped_last_text = match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            candidate_index,
            text,
            ..
        } => {
            assert_eq!(candidate_index, 1);
            text
        }
        other => panic!("unexpected prev-cycle response: {other:?}"),
    };
    assert_ne!(wrapped_last_text, first_text);

    send_client_message(
        &mut stdin,
        &ClientMessage::Cycle {
            session_id: "cycle-wrap-test".into(),
            direction: CycleDirection::Next,
        },
    )
    .await;

    match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            candidate_index,
            text,
            ..
        } => {
            assert_eq!(candidate_index, 0);
            assert_eq!(text, first_text);
        }
        other => panic!("unexpected next-cycle response: {other:?}"),
    }

    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_query_respects_disabled_cd_fallback() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let config_dir = config_root.join("shellsuggest");
    let data_root = tmp.path().join("data");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();

    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[cd]
fallback_mode = "disabled"
"#,
    )
    .unwrap();

    let mut child = spawn_query(&config_root, &data_root);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    send_client_message(
        &mut stdin,
        &ClientMessage::Suggest {
            request_id: 3,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: workspace.to_string_lossy().to_string(),
            session_id: "config-test".into(),
            last_command: None,
        },
    )
    .await;

    match read_query_message(&mut lines).await {
        DaemonMessage::Suggestion {
            request_id,
            candidate_count,
            text,
            ..
        } => {
            assert_eq!(request_id, 3);
            assert_eq!(candidate_count, 0);
            assert!(text.is_empty());
        }
        other => panic!("unexpected response: {other:?}"),
    }

    drop(stdin);
    let _ = child.kill().await;
}

#[test]
fn test_status_command_reports_metrics_and_config_snapshot() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let config_dir = config_root.join("shellsuggest");
    let data_root = tmp.path().join("data");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();

    std::fs::write(
        config_dir.join("config.toml"),
        r#"[path]
max_entries = 32
show_hidden = true

[cd]
fallback_mode = "disabled"

[ui]
max_candidates = 7
"#,
    )
    .unwrap();

    let store = open_store(&data_root);
    store
        .insert_journal(&JournalEntry {
            id: None,
            command_line: "make test".into(),
            cwd: tmp.path().to_string_lossy().to_string(),
            exit_code: Some(0),
            duration_ms: Some(12),
            session_id: "status-test".into(),
            timestamp: 1_000,
        })
        .unwrap();
    for source in ["history", "history", "cd_assist"] {
        store
            .insert_feedback(&FeedbackEntry {
                id: None,
                command_line: "make test".into(),
                source: source.into(),
                score: Some(0.9),
                accepted: true,
                session_id: "status-test".into(),
                timestamp: 2_000,
            })
            .unwrap();
    }
    store
        .insert_feedback(&FeedbackEntry {
            id: None,
            command_line: "make build".into(),
            source: "history".into(),
            score: Some(0.2),
            accepted: false,
            session_id: "status-test".into(),
            timestamp: 2_100,
        })
        .unwrap();
    store
        .upsert_path_cache(&PathCacheEntry {
            dir_path: tmp.path().to_string_lossy().to_string(),
            entries_json: r#"["src","scripts"]"#.into(),
            entry_count: 2,
            cached_at: 2_200,
        })
        .unwrap();

    let output = StdCommand::new(test_binary_path())
        .arg("status")
        .env("XDG_CONFIG_HOME", &config_root)
        .env("XDG_DATA_HOME", &data_root)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("runtime: per-shell query process (no shared daemon)"));
    assert!(stdout.contains(&format!("db: {}", db_path(&data_root).display())));
    assert!(stdout.contains("journal_rows: 1"));
    assert!(stdout.contains("feedback.accepted: 3"));
    assert!(stdout.contains("feedback.rejected: 1"));
    assert!(stdout.contains("feedback.acceptance_rate: 75.0%"));
    assert!(stdout.contains("feedback.by_source: history=2, cd_assist=1"));
    assert!(stdout.contains("path_cache_rows: 1"));
    assert!(stdout.contains("  path.max_entries = 32"));
    assert!(stdout.contains("  path.show_hidden = true"));
    assert!(stdout.contains("  cd.fallback_mode = disabled"));
    assert!(stdout.contains("  ui.max_candidates = 7"));
}

#[test]
fn test_runtime_commands_fail_fast_on_invalid_config() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let config_dir = config_root.join("shellsuggest");
    let data_root = tmp.path().join("data");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::write(config_dir.join("config.toml"), "not = [valid").unwrap();

    for command in ["query", "status", "journal"] {
        let output = StdCommand::new(test_binary_path())
            .arg(command)
            .env("XDG_CONFIG_HOME", &config_root)
            .env("XDG_DATA_HOME", &data_root)
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "expected `{command}` to fail on invalid config"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to parse config"),
            "unexpected stderr for `{command}`: {stderr}"
        );
    }
}
