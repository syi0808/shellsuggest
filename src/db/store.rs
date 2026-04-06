use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection};
use std::collections::HashMap;

use super::models::{
    FeedbackEntry, JournalEntry, PathCacheEntry, RankedCommand, SeededCommandStat,
};

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS command_journal (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                command_line TEXT NOT NULL,
                cwd         TEXT NOT NULL,
                exit_code   INTEGER,
                duration_ms INTEGER,
                session_id  TEXT NOT NULL,
                timestamp   INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_journal_command_line
                ON command_journal(command_line);
            CREATE INDEX IF NOT EXISTS idx_journal_command_line_timestamp
                ON command_journal(command_line, timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_journal_timestamp
                ON command_journal(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_journal_cwd
                ON command_journal(cwd);
            CREATE INDEX IF NOT EXISTS idx_journal_cwd_timestamp
                ON command_journal(cwd, timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_journal_cwd_command_line
                ON command_journal(cwd, command_line);

            CREATE TABLE IF NOT EXISTS suggestion_feedback (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                command_line TEXT NOT NULL,
                source      TEXT NOT NULL,
                score       REAL,
                accepted    INTEGER NOT NULL DEFAULT 0,
                session_id  TEXT NOT NULL,
                timestamp   INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_feedback_command_line
                ON suggestion_feedback(command_line);
            CREATE INDEX IF NOT EXISTS idx_feedback_accepted
                ON suggestion_feedback(accepted);
            CREATE INDEX IF NOT EXISTS idx_feedback_source_timestamp
                ON suggestion_feedback(source, timestamp DESC);

            CREATE TABLE IF NOT EXISTS path_cache (
                dir_path    TEXT PRIMARY KEY,
                entries_json TEXT NOT NULL,
                entry_count INTEGER NOT NULL,
                cached_at   INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS command_stats (
                command_line TEXT PRIMARY KEY,
                latest_timestamp INTEGER NOT NULL,
                success_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cwd_command_stats (
                cwd TEXT NOT NULL,
                command_line TEXT NOT NULL,
                latest_timestamp INTEGER NOT NULL,
                success_count INTEGER NOT NULL,
                PRIMARY KEY (cwd, command_line)
            );

            CREATE TABLE IF NOT EXISTS history_seed_stats (
                command_line TEXT PRIMARY KEY,
                latest_timestamp INTEGER NOT NULL,
                sample_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS transition_stats (
                prev_command TEXT NOT NULL,
                next_command TEXT NOT NULL,
                transition_count INTEGER NOT NULL,
                PRIMARY KEY (prev_command, next_command)
            );
        ",
        )?;
        self.backfill_aggregate_tables_if_needed()?;
        Ok(())
    }

    pub fn insert_journal(&self, entry: &JournalEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO command_journal (command_line, cwd, exit_code, duration_ms, session_id, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.command_line,
                entry.cwd,
                entry.exit_code,
                entry.duration_ms.map(|v| v as i64),
                entry.session_id,
                entry.timestamp as i64,
            ],
        )?;
        let new_id = self.conn.last_insert_rowid();
        if let Some(prev_command) = self.previous_command_in_session(&entry.session_id, new_id)? {
            self.conn.execute(
                "INSERT INTO transition_stats (prev_command, next_command, transition_count)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT(prev_command, next_command) DO UPDATE SET
                   transition_count = transition_stats.transition_count + 1",
                params![prev_command, entry.command_line],
            )?;
        }
        if matches!(entry.exit_code, None | Some(0)) {
            self.conn.execute(
                "INSERT INTO command_stats (command_line, latest_timestamp, success_count)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT(command_line) DO UPDATE SET
                   latest_timestamp = MAX(command_stats.latest_timestamp, excluded.latest_timestamp),
                   success_count = command_stats.success_count + 1",
                params![entry.command_line, entry.timestamp as i64],
            )?;
            self.conn.execute(
                "INSERT INTO cwd_command_stats (cwd, command_line, latest_timestamp, success_count)
                 VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(cwd, command_line) DO UPDATE SET
                   latest_timestamp = MAX(cwd_command_stats.latest_timestamp, excluded.latest_timestamp),
                   success_count = cwd_command_stats.success_count + 1",
                params![entry.cwd, entry.command_line, entry.timestamp as i64],
            )?;
        }
        Ok(())
    }

    pub fn query_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<JournalEntry>> {
        let pattern = format!("{}%", prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, command_line, cwd, exit_code, duration_ms, session_id, timestamp
             FROM command_journal
             WHERE command_line LIKE ?1
               AND (exit_code IS NULL OR exit_code = 0)
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;
        let entries = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok(JournalEntry {
                    id: row.get(0)?,
                    command_line: row.get(1)?,
                    cwd: row.get(2)?,
                    exit_code: row.get(3)?,
                    duration_ms: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                    session_id: row.get(5)?,
                    timestamp: row.get::<_, i64>(6)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    pub fn ranked_commands_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<RankedCommand>> {
        let upper = prefix_upper_bound(prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT command_line, latest_timestamp, success_count
             FROM command_stats
             WHERE command_line >= ?1
               AND command_line < ?2
             ORDER BY latest_timestamp DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![prefix, upper, limit as i64], |row| {
            Ok(RankedCommand {
                command_line: row.get(0)?,
                timestamp: row.get::<_, i64>(1)? as u64,
                frequency: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn seeded_commands_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<RankedCommand>> {
        let upper = prefix_upper_bound(prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT command_line, latest_timestamp, sample_count
             FROM history_seed_stats
             WHERE command_line >= ?1
               AND command_line < ?2
             ORDER BY latest_timestamp DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![prefix, upper, limit as i64], |row| {
            Ok(RankedCommand {
                command_line: row.get(0)?,
                timestamp: row.get::<_, i64>(1)? as u64,
                frequency: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn query_by_prefix_and_cwd(
        &self,
        prefix: &str,
        cwd: &str,
        limit: usize,
    ) -> Result<Vec<JournalEntry>> {
        let pattern = format!("{}%", prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, command_line, cwd, exit_code, duration_ms, session_id, timestamp
             FROM command_journal
             WHERE command_line LIKE ?1
               AND cwd = ?2
               AND (exit_code IS NULL OR exit_code = 0)
             ORDER BY timestamp DESC
             LIMIT ?3",
        )?;
        let entries = stmt
            .query_map(params![pattern, cwd, limit as i64], |row| {
                Ok(JournalEntry {
                    id: row.get(0)?,
                    command_line: row.get(1)?,
                    cwd: row.get(2)?,
                    exit_code: row.get(3)?,
                    duration_ms: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                    session_id: row.get(5)?,
                    timestamp: row.get::<_, i64>(6)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    pub fn ranked_commands_by_prefix_and_cwd(
        &self,
        prefix: &str,
        cwd: &str,
        limit: usize,
    ) -> Result<Vec<RankedCommand>> {
        let upper = prefix_upper_bound(prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT command_line, latest_timestamp, success_count
             FROM cwd_command_stats
             WHERE cwd = ?1
               AND command_line >= ?2
               AND command_line < ?3
             ORDER BY latest_timestamp DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![cwd, prefix, upper, limit as i64], |row| {
            Ok(RankedCommand {
                command_line: row.get(0)?,
                timestamp: row.get::<_, i64>(1)? as u64,
                frequency: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn has_ranked_command_prefix_and_cwd(&self, prefix: &str, cwd: &str) -> Result<bool> {
        let upper = prefix_upper_bound(prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT 1
             FROM cwd_command_stats
             WHERE cwd = ?1
               AND command_line >= ?2
               AND command_line < ?3
             LIMIT 1",
        )?;
        let result = stmt.query_row(params![cwd, prefix, upper], |_| Ok(()));
        match result {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub fn insert_feedback(&self, entry: &FeedbackEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO suggestion_feedback (command_line, source, score, accepted, session_id, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.command_line,
                entry.source,
                entry.score,
                entry.accepted as i32,
                entry.session_id,
                entry.timestamp as i64,
            ],
        )?;
        Ok(())
    }

    pub fn get_path_cache(
        &self,
        dir_path: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<Option<PathCacheEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_path, entries_json, entry_count, cached_at
             FROM path_cache
             WHERE dir_path = ?1",
        )?;
        let result = stmt.query_row(params![dir_path], |row| {
            Ok(PathCacheEntry {
                dir_path: row.get(0)?,
                entries_json: row.get(1)?,
                entry_count: row.get::<_, i64>(2)? as usize,
                cached_at: row.get::<_, i64>(3)? as u64,
            })
        });
        match result {
            Ok(entry) => {
                if now_ms.saturating_sub(entry.cached_at) <= ttl_ms {
                    Ok(Some(entry))
                } else {
                    Ok(None)
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn upsert_path_cache(&self, entry: &PathCacheEntry) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO path_cache (dir_path, entries_json, entry_count, cached_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        stmt.execute(params![
            entry.dir_path,
            entry.entries_json,
            entry.entry_count as i64,
            entry.cached_at as i64,
        ])?;
        Ok(())
    }

    pub fn recent_entries(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, command_line, cwd, exit_code, duration_ms, session_id, timestamp
             FROM command_journal
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(JournalEntry {
                id: row.get(0)?,
                command_line: row.get(1)?,
                cwd: row.get(2)?,
                exit_code: row.get(3)?,
                duration_ms: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                session_id: row.get(5)?,
                timestamp: row.get::<_, i64>(6)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn command_frequency(&self, command_line: &str) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM command_journal
             WHERE command_line = ?1
               AND (exit_code IS NULL OR exit_code = 0)",
            params![command_line],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    pub fn command_frequencies(&self, command_lines: &[String]) -> Result<HashMap<String, u64>> {
        if command_lines.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = std::iter::repeat("?")
            .take(command_lines.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT command_line, COUNT(*) as cnt
             FROM command_journal
             WHERE command_line IN ({placeholders})
               AND (exit_code IS NULL OR exit_code = 0)
             GROUP BY command_line"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params_from_iter(command_lines.iter().map(|command| command.as_str())),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
        )?;

        rows.collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(Into::into)
    }

    pub fn last_exit_code(&self, command_line: &str) -> Result<Option<i32>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT exit_code FROM command_journal
             WHERE command_line = ?1
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;
        let result = stmt.query_row(params![command_line], |row| row.get::<_, Option<i32>>(0));
        match result {
            Ok(code) => Ok(code),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn transition_count(
        &self,
        prev_command: &str,
        next_prefix: &str,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        let upper = prefix_upper_bound(next_prefix);
        let mut stmt = self.conn.prepare_cached(
            "SELECT next_command, transition_count
             FROM transition_stats
             WHERE prev_command = ?1
               AND next_command >= ?2
               AND next_command < ?3
             ORDER BY transition_count DESC
             LIMIT ?4",
        )?;
        let rows = stmt
            .query_map(
                params![prev_command, next_prefix, upper, limit as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn journal_count(&self) -> Result<u64> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM command_journal", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    pub fn feedback_counts(&self) -> Result<(u64, u64)> {
        let accepted: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM suggestion_feedback WHERE accepted = 1",
            [],
            |row| row.get(0),
        )?;
        let rejected: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM suggestion_feedback WHERE accepted = 0",
            [],
            |row| row.get(0),
        )?;
        Ok((accepted as u64, rejected as u64))
    }

    pub fn accepted_feedback_by_source(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, COUNT(*) as cnt
             FROM suggestion_feedback
             WHERE accepted = 1
             GROUP BY source
             ORDER BY cnt DESC, source ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn path_cache_count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM path_cache", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    pub fn replace_seeded_command_stats(&self, stats: &[SeededCommandStat]) -> Result<()> {
        self.conn.execute("DELETE FROM history_seed_stats", [])?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO history_seed_stats (command_line, latest_timestamp, sample_count)
             VALUES (?1, ?2, ?3)",
        )?;
        for stat in stats {
            stmt.execute(params![
                stat.command_line,
                stat.latest_timestamp as i64,
                stat.sample_count as i64,
            ])?;
        }
        Ok(())
    }

    pub fn seeded_command_count(&self) -> Result<u64> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM history_seed_stats", [], |row| {
                    row.get(0)
                })?;
        Ok(count as u64)
    }

    fn previous_command_in_session(
        &self,
        session_id: &str,
        before_id: i64,
    ) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT command_line
             FROM command_journal
             WHERE session_id = ?1
               AND id < ?2
             ORDER BY id DESC
             LIMIT 1",
        )?;
        let result = stmt.query_row(params![session_id, before_id], |row| row.get(0));
        match result {
            Ok(command) => Ok(Some(command)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn backfill_aggregate_tables_if_needed(&self) -> Result<()> {
        let command_stats_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM command_stats", [], |row| row.get(0))?;
        let cwd_stats_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM cwd_command_stats", [], |row| {
                    row.get(0)
                })?;
        let transition_stats_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM transition_stats", [], |row| {
                    row.get(0)
                })?;

        if command_stats_count == 0 {
            self.conn.execute(
                "INSERT OR REPLACE INTO command_stats (command_line, latest_timestamp, success_count)
                 SELECT command_line, MAX(timestamp), COUNT(*)
                 FROM command_journal
                 WHERE exit_code IS NULL OR exit_code = 0
                 GROUP BY command_line",
                [],
            )?;
        }
        if cwd_stats_count == 0 {
            self.conn.execute(
                "INSERT OR REPLACE INTO cwd_command_stats (cwd, command_line, latest_timestamp, success_count)
                 SELECT cwd, command_line, MAX(timestamp), COUNT(*)
                 FROM command_journal
                 WHERE exit_code IS NULL OR exit_code = 0
                 GROUP BY cwd, command_line",
                [],
            )?;
        }
        if transition_stats_count == 0 {
            self.conn.execute(
                "INSERT OR REPLACE INTO transition_stats (prev_command, next_command, transition_count)
                 SELECT j1.command_line, j2.command_line, COUNT(*)
                 FROM command_journal j1
                 JOIN command_journal j2
                   ON j2.id = j1.id + 1
                  AND j2.session_id = j1.session_id
                 GROUP BY j1.command_line, j2.command_line",
                [],
            )?;
        }
        Ok(())
    }
}

fn prefix_upper_bound(prefix: &str) -> String {
    format!("{prefix}\u{10FFFF}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_journal(
        command_line: &str,
        cwd: &str,
        exit_code: Option<i32>,
        session_id: &str,
        timestamp: u64,
    ) -> JournalEntry {
        JournalEntry {
            id: None,
            command_line: command_line.to_string(),
            cwd: cwd.to_string(),
            exit_code,
            duration_ms: None,
            session_id: session_id.to_string(),
            timestamp,
        }
    }

    #[test]
    fn test_insert_and_query_prefix() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "git status",
                "/home/user",
                Some(0),
                "sess1",
                100,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "git diff",
                "/home/user",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal("ls -la", "/home/user", Some(0), "sess1", 300))
            .unwrap();

        let results = store.query_by_prefix("git", 10).unwrap();
        assert_eq!(results.len(), 2);
        // ordered by timestamp DESC
        assert_eq!(results[0].command_line, "git diff");
        assert_eq!(results[1].command_line, "git status");
    }

    #[test]
    fn test_query_by_prefix_and_cwd() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "git status",
                "/project/a",
                Some(0),
                "sess1",
                100,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "git log",
                "/project/b",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "git diff",
                "/project/a",
                Some(0),
                "sess1",
                300,
            ))
            .unwrap();

        let results = store
            .query_by_prefix_and_cwd("git", "/project/a", 10)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.cwd == "/project/a"));
    }

    #[test]
    fn test_ranked_commands_by_prefix() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "cargo test",
                "/project",
                Some(0),
                "sess1",
                100,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "cargo test",
                "/project",
                Some(0),
                "sess1",
                300,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "cargo build",
                "/project",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();

        let ranked = store.ranked_commands_by_prefix("cargo ", 10).unwrap();
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].command_line, "cargo test");
        assert_eq!(ranked[0].timestamp, 300);
        assert_eq!(ranked[0].frequency, 2);
        assert_eq!(ranked[1].command_line, "cargo build");
    }

    #[test]
    fn test_ranked_commands_by_prefix_and_cwd() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "cargo test",
                "/project",
                Some(0),
                "sess1",
                100,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal("cargo test", "/other", Some(0), "sess1", 300))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "cargo build",
                "/project",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();

        let ranked = store
            .ranked_commands_by_prefix_and_cwd("cargo ", "/project", 10)
            .unwrap();
        assert_eq!(ranked.len(), 2);
        assert!(ranked
            .iter()
            .all(|entry| entry.command_line.starts_with("cargo ")));
        assert_eq!(ranked[0].command_line, "cargo build");
        assert_eq!(ranked[1].command_line, "cargo test");
    }

    #[test]
    fn test_has_ranked_command_prefix_and_cwd() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "cargo test",
                "/project",
                Some(0),
                "sess1",
                100,
            ))
            .unwrap();

        assert!(store
            .has_ranked_command_prefix_and_cwd("cargo ", "/project")
            .unwrap());
        assert!(!store
            .has_ranked_command_prefix_and_cwd("cargo ", "/other")
            .unwrap());
        assert!(!store
            .has_ranked_command_prefix_and_cwd("git ", "/project")
            .unwrap());
    }

    #[test]
    fn test_query_by_prefix_ignores_failed_commands() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal("npm test", "/project", Some(1), "sess1", 100))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "npm run lint",
                "/project",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();

        let results = store.query_by_prefix("npm", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command_line, "npm run lint");
    }

    #[test]
    fn test_command_frequency_ignores_failed_commands() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal(
                "make test",
                "/project",
                Some(1),
                "sess1",
                100,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "make test",
                "/project",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "make test",
                "/project",
                Some(0),
                "sess1",
                300,
            ))
            .unwrap();

        assert_eq!(store.command_frequency("make test").unwrap(), 2);
    }

    #[test]
    fn test_command_frequency() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal("ls", "/home", Some(0), "sess1", 1))
            .unwrap();
        store
            .insert_journal(&make_journal("ls", "/home", Some(0), "sess1", 2))
            .unwrap();
        store
            .insert_journal(&make_journal("ls", "/home", Some(0), "sess1", 3))
            .unwrap();
        store
            .insert_journal(&make_journal("pwd", "/home", Some(0), "sess1", 4))
            .unwrap();

        assert_eq!(store.command_frequency("ls").unwrap(), 3);
        assert_eq!(store.command_frequency("pwd").unwrap(), 1);
        assert_eq!(store.command_frequency("nonexistent").unwrap(), 0);
    }

    #[test]
    fn test_command_frequencies_batch() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal("cargo test", "/proj", Some(0), "sess1", 1))
            .unwrap();
        store
            .insert_journal(&make_journal("cargo test", "/proj", Some(0), "sess1", 2))
            .unwrap();
        store
            .insert_journal(&make_journal("cargo build", "/proj", Some(0), "sess1", 3))
            .unwrap();
        store
            .insert_journal(&make_journal("cargo build", "/proj", Some(1), "sess1", 4))
            .unwrap();

        let counts = store
            .command_frequencies(&["cargo test".to_string(), "cargo build".to_string()])
            .unwrap();

        assert_eq!(counts.get("cargo test"), Some(&2));
        assert_eq!(counts.get("cargo build"), Some(&1));
    }

    #[test]
    fn test_path_cache_ttl() {
        let store = Store::open_in_memory().unwrap();

        let entry = PathCacheEntry {
            dir_path: "/home/user".to_string(),
            entries_json: r#"["file1", "file2"]"#.to_string(),
            entry_count: 2,
            cached_at: 1000,
        };
        store.upsert_path_cache(&entry).unwrap();

        // Within TTL: now=1500, ttl=1000 => age=500 <= 1000
        let result = store.get_path_cache("/home/user", 1000, 1500).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().entry_count, 2);

        // Expired: now=3000, ttl=1000 => age=2000 > 1000
        let result = store.get_path_cache("/home/user", 1000, 3000).unwrap();
        assert!(result.is_none());

        // Non-existent path
        let result = store.get_path_cache("/nonexistent", 1000, 1500).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_feedback_metrics_queries() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_feedback(&FeedbackEntry {
                id: None,
                command_line: "echo hello".into(),
                source: "history".into(),
                score: Some(0.8),
                accepted: true,
                session_id: "sess1".into(),
                timestamp: 100,
            })
            .unwrap();
        store
            .insert_feedback(&FeedbackEntry {
                id: None,
                command_line: "vim main.rs".into(),
                source: "cwd_history".into(),
                score: Some(0.9),
                accepted: true,
                session_id: "sess1".into(),
                timestamp: 200,
            })
            .unwrap();
        store
            .insert_feedback(&FeedbackEntry {
                id: None,
                command_line: "cd src".into(),
                source: "cd_assist".into(),
                score: Some(0.6),
                accepted: false,
                session_id: "sess1".into(),
                timestamp: 300,
            })
            .unwrap();
        store
            .upsert_path_cache(&PathCacheEntry {
                dir_path: "/project".into(),
                entries_json: "[]".into(),
                entry_count: 0,
                cached_at: 10,
            })
            .unwrap();

        assert_eq!(store.journal_count().unwrap(), 0);
        assert_eq!(store.feedback_counts().unwrap(), (2, 1));
        assert_eq!(
            store.accepted_feedback_by_source().unwrap(),
            vec![("cwd_history".into(), 1), ("history".into(), 1)]
        );
        assert_eq!(store.path_cache_count().unwrap(), 1);
    }

    #[test]
    fn test_transition_count() {
        let store = Store::open_in_memory().unwrap();

        // Sequence: git add -> git commit (twice in same session)
        // We rely on AUTOINCREMENT id ordering, so insert in order
        store
            .insert_journal(&make_journal("git add .", "/proj", Some(0), "sess1", 100))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "git commit -m 'a'",
                "/proj",
                Some(0),
                "sess1",
                200,
            ))
            .unwrap();
        store
            .insert_journal(&make_journal("git add .", "/proj", Some(0), "sess1", 300))
            .unwrap();
        store
            .insert_journal(&make_journal(
                "git commit -m 'b'",
                "/proj",
                Some(0),
                "sess1",
                400,
            ))
            .unwrap();
        // Different session - should not count as transition
        store
            .insert_journal(&make_journal("git add .", "/proj", Some(0), "sess2", 500))
            .unwrap();
        store
            .insert_journal(&make_journal("git push", "/proj", Some(0), "sess2", 600))
            .unwrap();

        let transitions = store
            .transition_count("git add .", "git commit", 10)
            .unwrap();
        assert_eq!(transitions.len(), 2); // "git commit -m 'a'" and "git commit -m 'b'"
                                          // total counts across both entries should be 1 each (each appeared once after "git add .")
        assert!(transitions.iter().all(|(_, cnt)| *cnt == 1));
    }

    #[test]
    fn test_transition_count_backfills_existing_journal() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE command_journal (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                command_line TEXT NOT NULL,
                cwd         TEXT NOT NULL,
                exit_code   INTEGER,
                duration_ms INTEGER,
                session_id  TEXT NOT NULL,
                timestamp   INTEGER NOT NULL
            );
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO command_journal (command_line, cwd, exit_code, duration_ms, session_id, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["git add .", "/proj", 0, 1, "sess1", 100],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO command_journal (command_line, cwd, exit_code, duration_ms, session_id, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["git commit -m 'a'", "/proj", 0, 1, "sess1", 200],
        )
        .unwrap();

        let mut store = Store { conn };
        store.migrate().unwrap();

        let transitions = store
            .transition_count("git add .", "git commit", 10)
            .unwrap();
        assert_eq!(transitions, vec![("git commit -m 'a'".into(), 1)]);
    }

    #[test]
    fn test_last_exit_code() {
        let store = Store::open_in_memory().unwrap();

        store
            .insert_journal(&make_journal("make build", "/proj", Some(1), "sess1", 100))
            .unwrap();
        store
            .insert_journal(&make_journal("make build", "/proj", Some(0), "sess1", 200))
            .unwrap();

        // Should return the most recent exit code
        let code = store.last_exit_code("make build").unwrap();
        assert_eq!(code, Some(0));

        // Non-existent command
        let code = store.last_exit_code("nonexistent").unwrap();
        assert_eq!(code, None);
    }

    #[test]
    fn test_replace_and_query_seeded_command_stats() {
        let store = Store::open_in_memory().unwrap();

        store
            .replace_seeded_command_stats(&[
                SeededCommandStat {
                    command_line: "git status".into(),
                    latest_timestamp: 200,
                    sample_count: 3,
                },
                SeededCommandStat {
                    command_line: "git stash".into(),
                    latest_timestamp: 100,
                    sample_count: 1,
                },
            ])
            .unwrap();

        let seeded = store.seeded_commands_by_prefix("git st", 10).unwrap();
        assert_eq!(seeded.len(), 2);
        assert_eq!(seeded[0].command_line, "git status");
        assert_eq!(seeded[0].frequency, 3);
        assert_eq!(store.seeded_command_count().unwrap(), 2);

        store.replace_seeded_command_stats(&[]).unwrap();
        assert!(store
            .seeded_commands_by_prefix("git", 10)
            .unwrap()
            .is_empty());
        assert_eq!(store.seeded_command_count().unwrap(), 0);
    }
}
