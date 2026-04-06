#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub id: Option<i64>,
    pub command_line: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub session_id: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct FeedbackEntry {
    pub id: Option<i64>,
    pub command_line: String,
    pub source: String,
    pub score: Option<f32>,
    pub accepted: bool,
    pub session_id: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct PathCacheEntry {
    pub dir_path: String,
    pub entries_json: String,
    pub entry_count: usize,
    pub cached_at: u64,
}

#[derive(Debug, Clone)]
pub struct SeededCommandStat {
    pub command_line: String,
    pub latest_timestamp: u64,
    pub sample_count: u64,
}

#[derive(Debug, Clone)]
pub struct RankedCommand {
    pub command_line: String,
    pub timestamp: u64,
    pub frequency: u64,
}
