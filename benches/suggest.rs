use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use shellsuggest::config::{CdFallbackMode, CdMode};
use shellsuggest::daemon::broker::{Broker, BrokerOptions};
use shellsuggest::db::models::JournalEntry;
use shellsuggest::db::store::Store;
use shellsuggest::plugin::SuggestionRequest;
use shellsuggest::protocol::{self, ClientMessage, DaemonMessage};
use shellsuggest::runtime::QueryRuntime;

fn broker_options() -> BrokerOptions {
    BrokerOptions {
        cd_mode: CdMode::Builtin,
        path_show_hidden: false,
        path_max_entries: 256,
        max_candidates: 5,
        cd_fallback_mode: CdFallbackMode::CurrentDirOnly,
    }
}

fn populate_store(store: &Arc<Mutex<Store>>, count: usize) {
    let dirs = [
        "/project/src",
        "/project/tests",
        "/home/user",
        "/tmp",
        "/var/log",
    ];
    let commands = [
        "cd src",
        "cd tests",
        "vim main.rs",
        "cargo build",
        "cargo test",
        "make",
        "make test",
        "git status",
        "git diff",
        "ls -la",
        "docker compose up",
        "pnpm test",
    ];

    let s = store.lock().unwrap();
    for i in 0..count {
        let cmd = commands[i % commands.len()];
        let cwd = dirs[i % dirs.len()];
        s.insert_journal(&JournalEntry {
            id: None,
            command_line: cmd.into(),
            cwd: cwd.into(),
            exit_code: Some(0),
            duration_ms: Some(10),
            session_id: "bench".into(),
            timestamp: i as u64,
        })
        .unwrap();
    }
}

fn populate_transition_store(store: &Arc<Mutex<Store>>, pair_count: usize) {
    let s = store.lock().unwrap();
    for i in 0..pair_count {
        s.insert_journal(&JournalEntry {
            id: None,
            command_line: "vim main.rs".into(),
            cwd: "/project".into(),
            exit_code: Some(0),
            duration_ms: Some(10),
            session_id: "bench".into(),
            timestamp: (i * 2) as u64,
        })
        .unwrap();
        s.insert_journal(&JournalEntry {
            id: None,
            command_line: if i % 5 == 0 {
                "cargo build".into()
            } else {
                "cargo test".into()
            },
            cwd: "/project".into(),
            exit_code: Some(0),
            duration_ms: Some(10),
            session_id: "bench".into(),
            timestamp: (i * 2 + 1) as u64,
        })
        .unwrap();
    }
}

struct BenchQueryRuntime {
    runtime: QueryRuntime,
}

impl BenchQueryRuntime {
    fn start(row_count: usize) -> Self {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        populate_store(&store, row_count);
        Self {
            runtime: QueryRuntime::new(store, broker_options()),
        }
    }

    fn suggest(
        &mut self,
        buffer: &str,
        cursor: usize,
        cwd: &str,
        last_command: Option<&str>,
    ) -> DaemonMessage {
        let message = ClientMessage::Suggest {
            request_id: 1,
            buffer: buffer.to_string(),
            cursor,
            cwd: cwd.to_string(),
            session_id: "bench".into(),
            last_command: last_command.map(str::to_string),
        };

        let line = protocol::encode_client_message(&message);
        let parsed = protocol::parse_client_message(&line).unwrap();
        let response = self.runtime.handle_message(parsed);
        let encoded = protocol::encode_daemon_message(&response);
        protocol::parse_daemon_message(&encoded).unwrap()
    }
}

fn bench_suggest_10k(c: &mut Criterion) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    populate_store(&store, 10_000);
    let broker = Broker::new(store, broker_options());

    let req = SuggestionRequest {
        request_id: 1,
        buffer: "cd s".into(),
        cursor: 4,
        cwd: PathBuf::from("/project/src"),
        session_id: "bench".into(),
        last_command: None,
        timestamp_ms: 100_000,
    };

    c.bench_function("suggest_10k_rows", |b| {
        b.iter(|| broker.suggest(black_box(&req)))
    });
}

fn bench_suggest_100k(c: &mut Criterion) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    populate_store(&store, 100_000);
    let broker = Broker::new(store, broker_options());

    let req = SuggestionRequest {
        request_id: 1,
        buffer: "cargo ".into(),
        cursor: 6,
        cwd: PathBuf::from("/project"),
        session_id: "bench".into(),
        last_command: None,
        timestamp_ms: 200_000,
    };

    c.bench_function("suggest_100k_rows", |b| {
        b.iter(|| broker.suggest(black_box(&req)))
    });
}

fn bench_path_suggest(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().unwrap();
    for i in 0..256 {
        std::fs::create_dir(tmp.path().join(format!("dir_{i:03}"))).unwrap();
    }

    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let broker = Broker::new(store, broker_options());

    let req = SuggestionRequest {
        request_id: 1,
        buffer: "pushd d".into(),
        cursor: 7,
        cwd: tmp.path().to_path_buf(),
        session_id: "bench".into(),
        last_command: None,
        timestamp_ms: 100_000,
    };

    c.bench_function("path_plugin_suggest_256_entries", |b| {
        b.iter(|| broker.suggest(black_box(&req)))
    });
}

fn bench_transition_suggest_100k(c: &mut Criterion) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    populate_transition_store(&store, 50_000);
    let broker = Broker::new(store, broker_options());

    let req = SuggestionRequest {
        request_id: 1,
        buffer: "cargo ".into(),
        cursor: 6,
        cwd: PathBuf::from("/project"),
        session_id: "bench".into(),
        last_command: Some("vim main.rs".into()),
        timestamp_ms: 200_000,
    };

    c.bench_function("transition_suggest_100k_rows", |b| {
        b.iter(|| broker.suggest(black_box(&req)))
    });
}

fn bench_query_roundtrip_100k(c: &mut Criterion) {
    let mut runtime = BenchQueryRuntime::start(100_000);

    c.bench_function("query_roundtrip_100k_rows", |b| {
        b.iter(|| {
            let response = runtime.suggest(black_box("cargo "), 6, "/project", None);
            match response {
                DaemonMessage::Suggestion { text, .. } => {
                    assert_eq!(text, "cargo test");
                }
                other => panic!("unexpected query response: {other:?}"),
            }
        })
    });
}

criterion_group!(
    benches,
    bench_suggest_10k,
    bench_suggest_100k,
    bench_path_suggest,
    bench_transition_suggest_100k,
    bench_query_roundtrip_100k
);
criterion_main!(benches);
