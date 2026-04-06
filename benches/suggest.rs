use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use shellsuggest::config::CdFallbackMode;
use shellsuggest::daemon::broker::{Broker, BrokerOptions};
use shellsuggest::daemon::server;
use shellsuggest::db::models::JournalEntry;
use shellsuggest::db::store::Store;
use shellsuggest::plugin::SuggestionRequest;
use shellsuggest::protocol::{self, ClientMessage, DaemonMessage};

fn broker_options() -> BrokerOptions {
    BrokerOptions {
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

struct BenchDaemonClient {
    socket_path: PathBuf,
    _server_thread: std::thread::JoinHandle<()>,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl BenchDaemonClient {
    fn start(row_count: usize) -> Self {
        let socket_path = PathBuf::from(format!(
            "/tmp/shellsuggest-bench-{}-{}.sock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_file(socket_path.with_extension("pid"));
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        populate_store(&store, row_count);

        let server_thread = {
            let socket_path = socket_path.clone();
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(server::run(&socket_path, store, broker_options()))
                    .unwrap();
            })
        };

        let mut stream = None;
        for _ in 0..100 {
            match UnixStream::connect(&socket_path) {
                Ok(sock) => {
                    stream = Some(sock);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let writer = stream.expect("daemon socket should be reachable");
        writer
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        writer
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let reader = BufReader::new(writer.try_clone().unwrap());

        Self {
            socket_path,
            _server_thread: server_thread,
            reader,
            writer,
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

        let mut line = protocol::encode_client_message(&message);
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.flush().unwrap();

        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        protocol::parse_daemon_message(line.trim_end()).unwrap()
    }
}

impl Drop for BenchDaemonClient {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(self.socket_path.with_extension("pid"));
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

fn bench_daemon_roundtrip_100k(c: &mut Criterion) {
    let mut client = BenchDaemonClient::start(100_000);

    c.bench_function("daemon_roundtrip_100k_rows", |b| {
        b.iter(|| {
            let response = client.suggest(black_box("cargo "), 6, "/project", None);
            match response {
                DaemonMessage::Suggestion { text, .. } => {
                    assert_eq!(text, "cargo test");
                }
                other => panic!("unexpected daemon response: {other:?}"),
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
    bench_daemon_roundtrip_100k
);
criterion_main!(benches);
