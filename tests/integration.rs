use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use shellsuggest::config::CdFallbackMode;
use shellsuggest::daemon::broker::BrokerOptions;
use shellsuggest::db::models::{FeedbackEntry, JournalEntry, PathCacheEntry};
use shellsuggest::protocol::{self, ClientMessage, CycleDirection, DaemonMessage};
use std::process::{Command as StdCommand, Stdio};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::process::Command;

fn broker_options() -> BrokerOptions {
    BrokerOptions {
        path_show_hidden: false,
        path_max_entries: 256,
        max_candidates: 5,
        cd_fallback_mode: CdFallbackMode::CurrentDirOnly,
    }
}

fn test_binary_path() -> std::path::PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_shellsuggest") {
        return std::path::PathBuf::from(path);
    }

    let current = std::env::current_exe().unwrap();
    current
        .parent()
        .and_then(|path| path.parent())
        .unwrap()
        .join("shellsuggest")
}

async fn read_daemon_message(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> DaemonMessage {
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout waiting for line")
        .expect("io error")
        .expect("no line");
    protocol::parse_daemon_message(&line).unwrap()
}

async fn send_client_message(writer: &mut OwnedWriteHalf, message: &ClientMessage) {
    let mut line = protocol::encode_client_message(message);
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
}

async fn connect_with_retry(
    socket_path: &std::path::Path,
    daemon_handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
) -> UnixStream {
    for _ in 0..150 {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => return stream,
            Err(_) => {
                if daemon_handle.is_finished() {
                    panic!("daemon exited early");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    panic!("daemon did not start");
}

#[tokio::test]
async fn test_daemon_suggest_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let socket_path = std::path::PathBuf::from(format!(
        "/tmp/shellsuggest-test-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));

    std::fs::create_dir(tmp.path().join("testdir")).unwrap();
    std::fs::create_dir(tmp.path().join("testdir/src")).unwrap();
    std::fs::create_dir(tmp.path().join("testdir/scripts")).unwrap();

    let store = Arc::new(Mutex::new(
        shellsuggest::db::store::Store::open(db_path.to_str().unwrap()).unwrap(),
    ));

    // Insert some history
    store
        .lock()
        .unwrap()
        .insert_journal(&shellsuggest::db::models::JournalEntry {
            id: None,
            command_line: "cd src".into(),
            cwd: tmp.path().join("testdir").to_string_lossy().to_string(),
            exit_code: Some(0),
            duration_ms: Some(5),
            session_id: "test".into(),
            timestamp: 1000,
        })
        .unwrap();

    // Start daemon in background
    let socket_path_clone = socket_path.clone();
    let store_clone = Arc::clone(&store);
    let daemon_handle = tokio::spawn(async move {
        shellsuggest::daemon::server::run(&socket_path_clone, store_clone, broker_options()).await
    });

    // Wait for the daemon to accept connections.
    let stream = connect_with_retry(&socket_path, &daemon_handle).await;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    send_client_message(
        &mut writer,
        &ClientMessage::Suggest {
            request_id: 1,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: tmp.path().join("testdir").to_string_lossy().to_string(),
            session_id: "integration-test".into(),
            last_command: None,
        },
    )
    .await;

    let response = read_daemon_message(&mut lines).await;
    match response {
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

    // Test record command
    send_client_message(
        &mut writer,
        &ClientMessage::Record {
            command: "cd src".into(),
            cwd: tmp.path().join("testdir").to_string_lossy().to_string(),
            exit_code: 0,
            duration_ms: 5,
            session_id: "integration-test".into(),
        },
    )
    .await;

    assert_eq!(
        read_daemon_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );

    send_client_message(
        &mut writer,
        &ClientMessage::Record {
            command: "cd missing-dir".into(),
            cwd: tmp.path().join("testdir").to_string_lossy().to_string(),
            exit_code: 1,
            duration_ms: 5,
            session_id: "integration-test".into(),
        },
    )
    .await;

    assert_eq!(
        read_daemon_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );

    let successful = store.lock().unwrap().query_by_prefix("cd ", 10).unwrap();
    assert!(successful
        .iter()
        .any(|entry| entry.command_line == "cd src"));
    assert!(!successful
        .iter()
        .any(|entry| entry.command_line == "cd missing-dir"));

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
}

#[tokio::test]
async fn test_daemon_cycle_and_feedback_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let socket_path = std::path::PathBuf::from(format!(
        "/tmp/shellsuggest-cycle-test-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
    std::fs::create_dir(tmp.path().join("src")).unwrap();
    std::fs::create_dir(tmp.path().join("scripts")).unwrap();

    let store = Arc::new(Mutex::new(
        shellsuggest::db::store::Store::open_in_memory().unwrap(),
    ));
    let socket_path_clone = socket_path.clone();
    let store_clone = Arc::clone(&store);
    let daemon_handle = tokio::spawn(async move {
        shellsuggest::daemon::server::run(&socket_path_clone, store_clone, broker_options()).await
    });

    let stream = connect_with_retry(&socket_path, &daemon_handle).await;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    send_client_message(
        &mut writer,
        &ClientMessage::Suggest {
            request_id: 7,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: tmp.path().to_string_lossy().to_string(),
            session_id: "cycle-test".into(),
            last_command: None,
        },
    )
    .await;

    let first = read_daemon_message(&mut lines).await;
    let (second_text, second_source, second_score) = match first {
        DaemonMessage::Suggestion {
            request_id,
            candidate_count,
            candidate_index,
            ..
        } => {
            assert_eq!(candidate_count, 2);
            assert_eq!(candidate_index, 0);
            assert_eq!(request_id, 7);

            send_client_message(
                &mut writer,
                &ClientMessage::Cycle {
                    session_id: "cycle-test".into(),
                    direction: CycleDirection::Next,
                },
            )
            .await;

            match read_daemon_message(&mut lines).await {
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
        &mut writer,
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
        read_daemon_message(&mut lines).await,
        DaemonMessage::Ack { request_id: 0 }
    );
    assert_eq!(store.lock().unwrap().feedback_counts().unwrap(), (1, 0));

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
}

#[tokio::test]
async fn test_daemon_cycle_wraps_around_in_both_directions() {
    let tmp = TempDir::new().unwrap();
    let socket_path = std::path::PathBuf::from(format!(
        "/tmp/shellsuggest-cycle-wrap-test-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
    std::fs::create_dir(tmp.path().join("src")).unwrap();
    std::fs::create_dir(tmp.path().join("scripts")).unwrap();

    let store = Arc::new(Mutex::new(
        shellsuggest::db::store::Store::open_in_memory().unwrap(),
    ));
    let socket_path_clone = socket_path.clone();
    let store_clone = Arc::clone(&store);
    let daemon_handle = tokio::spawn(async move {
        shellsuggest::daemon::server::run(&socket_path_clone, store_clone, broker_options()).await
    });

    let stream = connect_with_retry(&socket_path, &daemon_handle).await;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    send_client_message(
        &mut writer,
        &ClientMessage::Suggest {
            request_id: 11,
            buffer: "cd s".into(),
            cursor: 4,
            cwd: tmp.path().to_string_lossy().to_string(),
            session_id: "cycle-wrap-test".into(),
            last_command: None,
        },
    )
    .await;

    let first_text = match read_daemon_message(&mut lines).await {
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
        &mut writer,
        &ClientMessage::Cycle {
            session_id: "cycle-wrap-test".into(),
            direction: CycleDirection::Prev,
        },
    )
    .await;

    let wrapped_last_text = match read_daemon_message(&mut lines).await {
        DaemonMessage::Suggestion {
            candidate_index, text, ..
        } => {
            assert_eq!(candidate_index, 1);
            text
        }
        other => panic!("unexpected prev-cycle response: {other:?}"),
    };
    assert_ne!(wrapped_last_text, first_text);

    send_client_message(
        &mut writer,
        &ClientMessage::Cycle {
            session_id: "cycle-wrap-test".into(),
            direction: CycleDirection::Next,
        },
    )
    .await;

    match read_daemon_message(&mut lines).await {
        DaemonMessage::Suggestion {
            candidate_index, text, ..
        } => {
            assert_eq!(candidate_index, 0);
            assert_eq!(text, first_text);
        }
        other => panic!("unexpected next-cycle response: {other:?}"),
    }

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
}

#[tokio::test]
async fn test_query_auto_starts_daemon_with_custom_socket_path() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let config_dir = config_root.join("shellsuggest");
    let data_root = tmp.path().join("data");
    let workspace = tmp.path().join("workspace");
    let socket_path = tmp.path().join("custom-shellsuggest.sock");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(workspace.join("src")).unwrap();

    std::fs::write(
        config_dir.join("config.toml"),
        format!("[daemon]\nsocket_path = \"{}\"\n", socket_path.display()),
    )
    .unwrap();

    let bin = test_binary_path();
    let mut child = Command::new(&bin)
        .arg("query")
        .env("XDG_CONFIG_HOME", &config_root)
        .env("XDG_DATA_HOME", &data_root)
        .env("SHELLSUGGEST_BIN", &bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    let mut msg = protocol::encode_client_message(&ClientMessage::Suggest {
        request_id: 1,
        buffer: "cd s".into(),
        cursor: 4,
        cwd: workspace.to_string_lossy().to_string(),
        session_id: "auto-start-test".into(),
        last_command: None,
    });
    msg.push('\n');
    stdin.write_all(msg.as_bytes()).await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("timeout waiting for query response")
        .expect("io error")
        .expect("no response");
    let response = protocol::parse_daemon_message(&response).unwrap();
    match response {
        DaemonMessage::Suggestion {
            request_id,
            source,
            text,
            ..
        } => {
            assert_eq!(request_id, 1);
            assert_eq!(source, "cd_assist");
            assert_eq!(text, "cd src");
        }
        other => panic!("unexpected response: {other:?}"),
    }
    assert!(socket_path.exists());

    drop(stdin);
    let _ = child.kill().await;

    let pid_path = socket_path.with_extension("pid");
    if let Ok(pid) = std::fs::read_to_string(&pid_path) {
        let _ = std::process::Command::new("kill").arg(pid.trim()).status();
    }
}

#[test]
fn test_status_command_reports_metrics_and_config_snapshot() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let config_dir = config_root.join("shellsuggest");
    let data_root = tmp.path().join("data");
    let data_dir = data_root.join("shellsuggest");
    let socket_path = tmp.path().join("status-shellsuggest.sock");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"[daemon]
socket_path = "{}"

[path]
max_entries = 32
show_hidden = true

[cd]
fallback_mode = "disabled"

[ui]
max_candidates = 7
"#,
            socket_path.display()
        ),
    )
    .unwrap();

    let db_path = data_dir.join("journal.db");
    let store = shellsuggest::db::store::Store::open(db_path.to_str().unwrap()).unwrap();
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
    assert!(stdout.contains("daemon: not running (no socket)"));
    assert!(stdout.contains(&format!("db: {}", db_path.display())));
    assert!(stdout.contains("journal_rows: 1"));
    assert!(stdout.contains("feedback.accepted: 3"));
    assert!(stdout.contains("feedback.rejected: 1"));
    assert!(stdout.contains("feedback.acceptance_rate: 75.0%"));
    assert!(stdout.contains("feedback.by_source: history=2, cd_assist=1"));
    assert!(stdout.contains("path_cache_rows: 1"));
    assert!(stdout.contains(&format!("  daemon.socket_path = {}", socket_path.display())));
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

    for command in ["serve", "query", "status", "journal"] {
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

#[tokio::test]
async fn test_daemon_echoes_client_request_ids_across_new_connections() {
    let tmp = TempDir::new().unwrap();
    let socket_path = std::path::PathBuf::from(format!(
        "/tmp/shellsuggest-request-id-test-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));

    let store = Arc::new(Mutex::new(
        shellsuggest::db::store::Store::open_in_memory().unwrap(),
    ));
    store
        .lock()
        .unwrap()
        .insert_journal(&JournalEntry {
            id: None,
            command_line: "echo hello".into(),
            cwd: tmp.path().to_string_lossy().to_string(),
            exit_code: Some(0),
            duration_ms: Some(5),
            session_id: "request-id-test".into(),
            timestamp: 1_000,
        })
        .unwrap();

    let socket_path_clone = socket_path.clone();
    let store_clone = Arc::clone(&store);
    let daemon_handle = tokio::spawn(async move {
        shellsuggest::daemon::server::run(&socket_path_clone, store_clone, broker_options()).await
    });

    let stream_a = connect_with_retry(&socket_path, &daemon_handle).await;
    let (reader_a, mut writer_a) = stream_a.into_split();
    let mut lines_a = BufReader::new(reader_a).lines();

    send_client_message(
        &mut writer_a,
        &ClientMessage::Suggest {
            request_id: 41,
            buffer: "ec".into(),
            cursor: 2,
            cwd: tmp.path().to_string_lossy().to_string(),
            session_id: "request-id-a".into(),
            last_command: None,
        },
    )
    .await;

    match read_daemon_message(&mut lines_a).await {
        DaemonMessage::Suggestion {
            request_id, text, ..
        } => {
            assert_eq!(request_id, 41);
            assert_eq!(text, "echo hello");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    let stream_b = UnixStream::connect(&socket_path).await.unwrap();
    let (reader_b, mut writer_b) = stream_b.into_split();
    let mut lines_b = BufReader::new(reader_b).lines();

    send_client_message(
        &mut writer_b,
        &ClientMessage::Suggest {
            request_id: 1,
            buffer: "ec".into(),
            cursor: 2,
            cwd: tmp.path().to_string_lossy().to_string(),
            session_id: "request-id-b".into(),
            last_command: None,
        },
    )
    .await;

    match read_daemon_message(&mut lines_b).await {
        DaemonMessage::Suggestion {
            request_id, text, ..
        } => {
            assert_eq!(request_id, 1);
            assert_eq!(text, "echo hello");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    daemon_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(socket_path.with_extension("pid"));
}
