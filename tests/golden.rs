use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use shellsuggest::config::CdFallbackMode;
use shellsuggest::daemon::broker::Broker;
use shellsuggest::daemon::broker::BrokerOptions;
use shellsuggest::db::models::JournalEntry;
use shellsuggest::db::store::Store;
use shellsuggest::plugin::SuggestionRequest;
use tempfile::TempDir;

fn setup_store(entries: Vec<(&str, &str, u64)>) -> Arc<Mutex<Store>> {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    for (cmd, cwd, ts) in entries {
        store
            .lock()
            .unwrap()
            .insert_journal(&JournalEntry {
                id: None,
                command_line: cmd.into(),
                cwd: cwd.into(),
                exit_code: Some(0),
                duration_ms: Some(10),
                session_id: "golden".into(),
                timestamp: ts,
            })
            .unwrap();
    }
    store
}

fn make_req(buffer: &str, cwd: &str, last_cmd: Option<&str>, ts: u64) -> SuggestionRequest {
    SuggestionRequest {
        request_id: 1,
        buffer: buffer.into(),
        cursor: buffer.len(),
        cwd: PathBuf::from(cwd),
        session_id: "golden".into(),
        last_command: last_cmd.map(String::from),
        timestamp_ms: ts,
    }
}

fn broker_options() -> BrokerOptions {
    BrokerOptions {
        path_show_hidden: false,
        path_max_entries: 256,
        max_candidates: 5,
        cd_fallback_mode: CdFallbackMode::CurrentDirOnly,
    }
}

#[test]
fn golden_cwd_aware_cd() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("src")).unwrap();
    std::fs::create_dir(tmp.path().join("scripts")).unwrap();

    let cwd = tmp.path().to_string_lossy().to_string();
    let store = setup_store(vec![
        ("cd src", &cwd, 1000),
        ("cd scripts", &cwd, 2000),
        ("cd somewhere", "/other", 3000),
    ]);

    let broker = Broker::new(store, broker_options());
    let req = make_req("cd s", &cwd, None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "cwd_aware_cd",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
            "score": result.as_ref().map(|c| (c.score * 1000.0).round() / 1000.0),
        })
    );
}

#[test]
fn golden_cd_requires_current_pwd_history() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("src")).unwrap();

    let cwd = tmp.path().to_string_lossy().to_string();
    let store = setup_store(vec![("cd src", "/other", 1000)]);

    let broker = Broker::new(store, broker_options());
    let req = make_req("cd s", &cwd, None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "cd_requires_current_pwd_history",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
        })
    );
}

#[test]
fn golden_history_fallback() {
    let store = setup_store(vec![
        ("docker compose up", "/any", 1000),
        ("docker compose down", "/any", 2000),
    ]);

    let broker = Broker::new(store, broker_options());
    let req = make_req("docker comp", "/unrelated", None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "history_fallback",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
        })
    );
}

#[test]
fn golden_vim_file_match() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("readme.md"), "hello").unwrap();
    std::fs::write(tmp.path().join("requirements.txt"), "").unwrap();

    let cwd = tmp.path().to_string_lossy().to_string();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let broker = Broker::new(store, broker_options());
    let req = make_req("vim rea", &cwd, None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "vim_file_match",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
        })
    );
}

#[test]
fn golden_cwd_history_priority() {
    let store = setup_store(vec![
        ("make test", "/project", 1000),
        ("make test", "/project", 2000),
        ("make test", "/project", 3000),
        ("make build", "/other", 4000),
        ("make deploy", "/other", 5000),
    ]);

    let broker = Broker::new(store, broker_options());
    let req = make_req("make ", "/project", None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "cwd_history_priority",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
        })
    );
}

#[test]
fn golden_no_match() {
    let store = setup_store(vec![("cd src", "/project", 1000)]);
    let broker = Broker::new(store, broker_options());
    let req = make_req("zzz_nonexistent", "/project", None, 10_000);
    let results = broker.suggest(&req);
    let result = results.first();

    insta::assert_json_snapshot!(
        "no_match",
        serde_json::json!({
            "suggestion": result.as_ref().map(|c| &c.text),
            "source": result.as_ref().map(|c| &c.source),
        })
    );
}
