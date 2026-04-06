use crate::plugin::SuggestionCandidate;
use std::path::Path;

/// Commands that expect a directory path argument
const DIR_COMMANDS: &[&str] = &["cd", "pushd", "ls"];

/// Commands that expect a file path argument
const FILE_COMMANDS: &[&str] = &["vim", "nvim", "code", "open", "cat", "less", "head", "tail"];

/// Commands that are dangerous — require higher confidence
const DANGEROUS_COMMANDS: &[&str] = &["rm", "cp", "mv"];

/// Minimum score for dangerous commands to be suggested
const DANGEROUS_THRESHOLD: f32 = 0.6;

pub struct RankingFeatures {
    pub prefix_exactness: f32,
    pub cwd_similarity: f32,
    pub path_exists: f32,
    pub recency: f32,
    pub frequency: f32,
    pub last_command_transition: f32,
    pub success_bonus: f32,
}

impl RankingFeatures {
    pub fn score(&self) -> f32 {
        0.30 * self.prefix_exactness
            + 0.25 * self.cwd_similarity
            + 0.20 * self.path_exists
            + 0.10 * self.recency
            + 0.05 * self.frequency
            + 0.05 * self.last_command_transition
            + 0.05 * self.success_bonus
    }
}

pub fn extract_command(buffer: &str) -> Option<&str> {
    buffer.split_whitespace().next()
}

pub fn extract_current_token(buffer: &str) -> &str {
    buffer.split_whitespace().last().unwrap_or("")
}

pub fn expects_directory(cmd: &str) -> bool {
    DIR_COMMANDS.contains(&cmd)
}

pub fn expects_file(cmd: &str) -> bool {
    FILE_COMMANDS.contains(&cmd)
}

pub fn is_dangerous(cmd: &str) -> bool {
    DANGEROUS_COMMANDS.contains(&cmd)
}

pub fn prefix_exactness(candidate: &str, typed: &str) -> f32 {
    if typed.is_empty() {
        return 0.0;
    }
    if candidate.starts_with(typed) {
        typed.len() as f32 / candidate.len().max(1) as f32
    } else {
        0.0
    }
}

pub fn cwd_similarity(candidate_cwd: &str, current_cwd: &str) -> f32 {
    if candidate_cwd == current_cwd {
        1.0
    } else if current_cwd.starts_with(candidate_cwd) || candidate_cwd.starts_with(current_cwd) {
        0.5
    } else {
        0.0
    }
}

pub fn recency(candidate_ts: u64, now_ts: u64) -> f32 {
    if candidate_ts >= now_ts {
        return 1.0;
    }
    let age_ms = now_ts - candidate_ts;
    let half_life_ms: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
    let decay = (-0.693 * age_ms as f64 / half_life_ms).exp();
    decay as f32
}

pub fn frequency(count: u64) -> f32 {
    (count as f32 / 100.0).min(1.0)
}

pub fn path_exists_score(candidate: &str, current_cwd: &Path) -> f32 {
    let mut parts = candidate.splitn(2, char::is_whitespace);
    let Some(cmd) = parts.next() else {
        return 0.0;
    };
    if !expects_directory(cmd) && !expects_file(cmd) {
        return 0.0;
    }

    let Some(arg) = parts.next() else {
        return 0.0;
    };
    let arg = arg.trim();
    if arg.is_empty() {
        return 0.0;
    }

    let path = if arg.starts_with('/') || arg.starts_with('~') {
        let expanded = if let Some(rest) = arg.strip_prefix('~') {
            if let Some(home) = dirs_or_cwd_home() {
                format!("{home}{rest}")
            } else {
                return 0.0;
            }
        } else {
            arg.to_string()
        };
        std::path::PathBuf::from(expanded)
    } else {
        current_cwd.join(arg)
    };

    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return 0.0,
    };

    if (expects_directory(cmd) && metadata.is_dir()) || (expects_file(cmd) && metadata.is_file()) {
        1.0
    } else {
        0.3
    }
}

fn dirs_or_cwd_home() -> Option<String> {
    std::env::var("HOME").ok()
}

pub fn apply_dangerous_filter(candidates: &mut Vec<SuggestionCandidate>) {
    candidates.retain(|c| {
        if let Some(cmd) = extract_command(&c.text) {
            if is_dangerous(cmd) && c.score < DANGEROUS_THRESHOLD {
                return false;
            }
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::SuggestionKind;

    #[test]
    fn test_score_weights_sum_to_one() {
        let features = RankingFeatures {
            prefix_exactness: 1.0,
            cwd_similarity: 1.0,
            path_exists: 1.0,
            recency: 1.0,
            frequency: 1.0,
            last_command_transition: 1.0,
            success_bonus: 1.0,
        };
        let score = features.score();
        assert!(
            (score - 1.0).abs() < 0.001,
            "weights should sum to 1.0, got {score}"
        );
    }

    #[test]
    fn test_prefix_exactness() {
        assert!(prefix_exactness("cd src", "cd s") > 0.0);
        assert_eq!(prefix_exactness("cd src", "cd xr"), 0.0);
        assert!(prefix_exactness("cd src", "cd src") > prefix_exactness("cd src", "cd s"));
        assert_eq!(prefix_exactness("anything", ""), 0.0);
    }

    #[test]
    fn test_cwd_similarity() {
        assert_eq!(cwd_similarity("/project/src", "/project/src"), 1.0);
        assert_eq!(cwd_similarity("/project", "/project/src"), 0.5);
        assert_eq!(cwd_similarity("/project/src", "/project"), 0.5);
        assert_eq!(cwd_similarity("/other", "/project"), 0.0);
    }

    #[test]
    fn test_recency_decay() {
        let now = 1_000_000_000u64;
        assert_eq!(recency(now, now), 1.0);
        assert!(recency(now - 1000, now) > recency(now - 100_000, now));
        let day_ms = 24 * 60 * 60 * 1000;
        let score = recency(now - day_ms, now + day_ms);
        assert!(score < 0.6 && score > 0.1);
    }

    #[test]
    fn test_frequency_capped() {
        assert_eq!(frequency(0), 0.0);
        assert_eq!(frequency(50), 0.5);
        assert_eq!(frequency(100), 1.0);
        assert_eq!(frequency(200), 1.0);
    }

    #[test]
    fn test_extract_command() {
        assert_eq!(extract_command("cd src"), Some("cd"));
        assert_eq!(extract_command("vim"), Some("vim"));
        assert_eq!(extract_command(""), None);
    }

    #[test]
    fn test_path_exists_score_skips_non_path_commands() {
        assert_eq!(path_exists_score("make test", Path::new("/tmp")), 0.0);
    }

    #[test]
    fn test_dangerous_filter() {
        let mut candidates = vec![
            SuggestionCandidate {
                text: "rm -rf /tmp/old".into(),
                source: "history".into(),
                score: 0.3,
                kind: SuggestionKind::Command,
            },
            SuggestionCandidate {
                text: "cd src".into(),
                source: "history".into(),
                score: 0.3,
                kind: SuggestionKind::Command,
            },
            SuggestionCandidate {
                text: "rm old.txt".into(),
                source: "history".into(),
                score: 0.7,
                kind: SuggestionKind::Command,
            },
        ];
        apply_dangerous_filter(&mut candidates);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].text, "cd src");
        assert_eq!(candidates[1].text, "rm old.txt");
    }

    #[test]
    fn test_command_type_detection() {
        assert!(expects_directory("cd"));
        assert!(!expects_directory("vim"));
        assert!(expects_file("vim"));
        assert!(expects_file("cat"));
        assert!(!expects_file("cd"));
        assert!(is_dangerous("rm"));
        assert!(!is_dangerous("ls"));
    }
}
