pub mod cd_assist;
pub mod cwd_history;
pub mod history;
pub mod path;

use smallvec::SmallVec;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum SuggestionKind {
    Command,
    Path,
    Argument,
}

#[derive(Debug, Clone)]
pub struct SuggestionRequest {
    pub request_id: u64,
    pub buffer: String,
    pub cursor: usize,
    pub cwd: PathBuf,
    pub session_id: String,
    pub last_command: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionCandidate {
    pub text: String,
    pub source: String,
    pub score: f32,
    pub kind: SuggestionKind,
}

pub trait InlineProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, req: &SuggestionRequest) -> bool;
    fn suggest(&self, req: &SuggestionRequest) -> SmallVec<[SuggestionCandidate; 4]>;
}
