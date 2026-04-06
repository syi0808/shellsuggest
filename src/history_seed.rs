use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::Config;
use crate::db::models::SeededCommandStat;
use crate::db::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHistoryLine {
    command_line: String,
    timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct HistorySeedSummary {
    pub source_path: Option<PathBuf>,
    pub parsed_entries: usize,
    pub imported_commands: usize,
}

pub fn prime_store_from_histfile(store: &Store, config: &Config) -> Result<HistorySeedSummary> {
    let Some(path) = config.history_seed_path() else {
        store.replace_seeded_command_stats(&[])?;
        return Ok(HistorySeedSummary {
            source_path: None,
            parsed_entries: 0,
            imported_commands: 0,
        });
    };

    if !path.exists() {
        store.replace_seeded_command_stats(&[])?;
        return Ok(HistorySeedSummary {
            source_path: Some(path),
            parsed_entries: 0,
            imported_commands: 0,
        });
    }

    let (stats, parsed_entries) = collect_seeded_stats(&path, config.history.seed_max_entries)?;
    store.replace_seeded_command_stats(&stats)?;
    Ok(HistorySeedSummary {
        source_path: Some(path),
        parsed_entries,
        imported_commands: stats.len(),
    })
}

fn collect_seeded_stats(
    path: &Path,
    max_entries: usize,
) -> Result<(Vec<SeededCommandStat>, usize)> {
    let file = File::open(path)
        .with_context(|| format!("failed to open history seed source {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut recent_entries = VecDeque::new();

    for line in reader.lines() {
        let line = line?;
        let Some(parsed) = parse_history_line(&line) else {
            continue;
        };
        recent_entries.push_back(parsed);
        while recent_entries.len() > max_entries {
            recent_entries.pop_front();
        }
    }

    let anchor_ms = file_anchor_ms(path);
    let total = recent_entries.len();
    let mut aggregated = HashMap::<String, SeededCommandStat>::new();

    for (idx, entry) in recent_entries.into_iter().enumerate() {
        let synthetic_offset = total.saturating_sub(idx + 1) as u64;
        let timestamp = entry
            .timestamp_ms
            .unwrap_or_else(|| anchor_ms.saturating_sub(synthetic_offset * 1_000));

        let stat = aggregated
            .entry(entry.command_line.clone())
            .or_insert_with(|| SeededCommandStat {
                command_line: entry.command_line,
                latest_timestamp: timestamp,
                sample_count: 0,
            });
        stat.latest_timestamp = stat.latest_timestamp.max(timestamp);
        stat.sample_count += 1;
    }

    let mut stats = aggregated.into_values().collect::<Vec<_>>();
    stats.sort_by(|a, b| b.latest_timestamp.cmp(&a.latest_timestamp));
    Ok((stats, total))
}

fn parse_history_line(line: &str) -> Option<ParsedHistoryLine> {
    if line.trim().is_empty() {
        return None;
    }

    if let Some(rest) = line.strip_prefix(": ") {
        if let Some((header, command)) = rest.split_once(';') {
            let timestamp = header
                .split(':')
                .next()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(|secs| secs.saturating_mul(1_000));

            if let Some(timestamp_ms) = timestamp {
                let command_line = command.trim();
                if !command_line.is_empty() {
                    return Some(ParsedHistoryLine {
                        command_line: command_line.into(),
                        timestamp_ms: Some(timestamp_ms),
                    });
                }
            }
        }
    }

    Some(ParsedHistoryLine {
        command_line: line.to_string(),
        timestamp_ms: None,
    })
}

fn file_anchor_ms(path: &Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_else(now_ms)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_extended_history_line() {
        let parsed = parse_history_line(": 1712460000:3;git status").unwrap();
        assert_eq!(
            parsed,
            ParsedHistoryLine {
                command_line: "git status".into(),
                timestamp_ms: Some(1_712_460_000_000),
            }
        );
    }

    #[test]
    fn test_parse_plain_history_line() {
        let parsed = parse_history_line("cargo test").unwrap();
        assert_eq!(
            parsed,
            ParsedHistoryLine {
                command_line: "cargo test".into(),
                timestamp_ms: None,
            }
        );
    }

    #[test]
    fn test_collect_seeded_stats_aggregates_recent_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        std::fs::write(
            &path,
            "\
: 1712460000:0;git status
cargo test
: 1712460300:0;git status
cargo test
",
        )
        .unwrap();

        let (stats, parsed_entries) = collect_seeded_stats(&path, 10).unwrap();
        assert_eq!(parsed_entries, 4);
        assert_eq!(stats.len(), 2);

        let git = stats
            .iter()
            .find(|stat| stat.command_line == "git status")
            .unwrap();
        assert_eq!(git.sample_count, 2);
        assert_eq!(git.latest_timestamp, 1_712_460_300_000);

        let cargo = stats
            .iter()
            .find(|stat| stat.command_line == "cargo test")
            .unwrap();
        assert_eq!(cargo.sample_count, 2);
        assert!(cargo.latest_timestamp > 0);
    }

    #[test]
    fn test_collect_seeded_stats_respects_max_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        std::fs::write(
            &path,
            "\
: 1712460000:0;echo dropped
: 1712460100:0;echo kept-one
: 1712460200:0;echo kept-two
",
        )
        .unwrap();

        let (stats, parsed_entries) = collect_seeded_stats(&path, 2).unwrap();
        assert_eq!(parsed_entries, 2);
        assert_eq!(stats.len(), 2);
        assert!(stats.iter().all(|stat| stat.command_line != "echo dropped"));
        assert!(stats
            .iter()
            .any(|stat| stat.command_line == "echo kept-one"));
        assert!(stats
            .iter()
            .any(|stat| stat.command_line == "echo kept-two"));
    }

    #[test]
    fn test_prime_store_clears_seeded_stats_when_disabled() {
        let store = Store::open_in_memory().unwrap();
        store
            .replace_seeded_command_stats(&[SeededCommandStat {
                command_line: "git status".into(),
                latest_timestamp: 10,
                sample_count: 1,
            }])
            .unwrap();

        let config = Config {
            history: crate::config::HistoryConfig {
                seed_from_histfile: false,
                histfile_path: String::new(),
                seed_max_entries: 100,
            },
            ..Config::default()
        };

        let summary = prime_store_from_histfile(&store, &config).unwrap();
        assert!(summary.source_path.is_none());
        assert_eq!(store.seeded_command_count().unwrap(), 0);
    }
}
