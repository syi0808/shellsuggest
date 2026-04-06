use smallvec::SmallVec;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::db::models::PathCacheEntry;
use crate::db::store::Store;
use crate::plugin::{InlineProvider, SuggestionCandidate, SuggestionKind, SuggestionRequest};
use crate::ranking;

const CACHE_TTL_MS: u64 = 5 * 60 * 1000;

const PATH_COMMANDS: &[&str] = &[
    "ls", "vim", "nvim", "code", "open", "cat", "less", "head", "tail", "pushd",
];

pub struct PathPlugin {
    store: Arc<Mutex<Store>>,
    memory_cache: Mutex<HashMap<String, MemoryCacheEntry>>,
    show_hidden: bool,
    max_entries: usize,
}

impl PathPlugin {
    pub fn new(store: Arc<Mutex<Store>>, show_hidden: bool, max_entries: usize) -> Self {
        Self {
            store,
            memory_cache: Mutex::new(HashMap::new()),
            show_hidden,
            max_entries,
        }
    }

    fn is_path_token(token: &str) -> bool {
        token.starts_with("./")
            || token.starts_with("../")
            || token.starts_with('/')
            || token.starts_with("~/")
    }

    fn resolve_dir_and_prefix<'a>(
        token: &'a str,
        cwd: &Path,
    ) -> Option<(std::path::PathBuf, &'a str)> {
        if token.is_empty() {
            return Some((cwd.to_path_buf(), ""));
        }

        if let Some(slash_pos) = token.rfind('/') {
            let dir_part = &token[..=slash_pos];
            let file_prefix = &token[slash_pos + 1..];

            let dir = if dir_part.starts_with('~') {
                let home = std::env::var("HOME").ok()?;
                let rest = dir_part.strip_prefix('~').unwrap_or(dir_part);
                std::path::PathBuf::from(format!("{home}{rest}"))
            } else if dir_part.starts_with('/') {
                std::path::PathBuf::from(dir_part)
            } else {
                cwd.join(dir_part)
            };

            Some((dir, file_prefix))
        } else {
            Some((cwd.to_path_buf(), token))
        }
    }

    fn read_dir_entries(&self, dir: &Path, now_ms: u64) -> Arc<[DirEntry]> {
        let dir_str = dir.to_string_lossy().to_string();

        if let Ok(cache) = self.memory_cache.lock() {
            if let Some(cached) = cache.get(&dir_str) {
                if now_ms.saturating_sub(cached.cached_at) <= CACHE_TTL_MS {
                    return Arc::clone(&cached.entries);
                }
            }
        }

        // Check cache
        let cached = {
            let store = self.store.lock().unwrap();
            store
                .get_path_cache(&dir_str, CACHE_TTL_MS, now_ms)
                .ok()
                .flatten()
        };
        if let Some(cached) = cached {
            if let Ok(names) = serde_json::from_str::<Vec<CachedEntry>>(&cached.entries_json) {
                let entries: Arc<[DirEntry]> = names
                    .into_iter()
                    .take(self.max_entries)
                    .map(|e| DirEntry {
                        name: e.name,
                        is_dir: e.is_dir,
                    })
                    .collect::<Vec<_>>()
                    .into();
                if let Ok(mut cache) = self.memory_cache.lock() {
                    cache.insert(
                        dir_str.clone(),
                        MemoryCacheEntry {
                            entries: Arc::clone(&entries),
                            cached_at: now_ms,
                        },
                    );
                }
                return entries;
            }
        }

        // Read from filesystem
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::<DirEntry>::new().into(),
        };

        let mut entries: Vec<DirEntry> = Vec::new();
        let mut count = 0usize;

        for entry in read_dir {
            count += 1;
            if count > self.max_entries {
                break;
            }
            if let Ok(entry) = entry {
                let name = entry.file_name().to_string_lossy().to_string();
                if !self.show_hidden && name.starts_with('.') {
                    continue;
                }
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                entries.push(DirEntry { name, is_dir });
            }
        }

        // Cache the results
        let cached: Vec<CachedEntry> = entries
            .iter()
            .map(|e| CachedEntry {
                name: e.name.clone(),
                is_dir: e.is_dir,
            })
            .collect();
        let entries: Arc<[DirEntry]> = entries.into();

        if let Ok(json) = serde_json::to_string(&cached) {
            let store = self.store.lock().unwrap();
            let _ = store.upsert_path_cache(&PathCacheEntry {
                dir_path: dir_str.clone(),
                entries_json: json,
                entry_count: entries.len(),
                cached_at: now_ms,
            });
        }

        if let Ok(mut cache) = self.memory_cache.lock() {
            cache.insert(
                dir_str,
                MemoryCacheEntry {
                    entries: Arc::clone(&entries),
                    cached_at: now_ms,
                },
            );
        }

        entries
    }
}

#[derive(Debug)]
struct DirEntry {
    name: String,
    is_dir: bool,
}

struct MemoryCacheEntry {
    entries: Arc<[DirEntry]>,
    cached_at: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedEntry {
    name: String,
    is_dir: bool,
}

impl InlineProvider for PathPlugin {
    fn name(&self) -> &'static str {
        "path"
    }

    fn supports(&self, req: &SuggestionRequest) -> bool {
        if req.buffer.is_empty() {
            return false;
        }

        let parts: SmallVec<[&str; 8]> = req.buffer.split_whitespace().collect();
        let trailing_space = req.buffer.ends_with(' ');

        match parts.len() {
            0 => false,
            1 => {
                if trailing_space {
                    // "cmd " — check if cmd is a path command
                    PATH_COMMANDS.contains(&parts[0])
                } else {
                    Self::is_path_token(parts[0])
                }
            }
            _ => {
                let cmd = parts[0];
                let last_token = parts.last().unwrap_or(&"");
                PATH_COMMANDS.contains(&cmd) || Self::is_path_token(last_token)
            }
        }
    }

    fn suggest(&self, req: &SuggestionRequest) -> SmallVec<[SuggestionCandidate; 4]> {
        let parts: SmallVec<[&str; 8]> = req.buffer.split_whitespace().collect();
        if parts.is_empty() {
            return SmallVec::new();
        }

        let trailing_space = req.buffer.ends_with(' ');

        let (cmd, token) = if parts.len() == 1 {
            if trailing_space {
                // "cmd " — cmd is parts[0], no token yet
                (parts[0], "")
            } else {
                ("", parts[0])
            }
        } else if trailing_space {
            (parts[0], "")
        } else {
            (parts[0], *parts.last().unwrap_or(&""))
        };

        let (dir, prefix) = match Self::resolve_dir_and_prefix(token, &req.cwd) {
            Some(dp) => dp,
            None => return SmallVec::new(),
        };

        let entries = self.read_dir_entries(&dir, req.timestamp_ms);
        let wants_dir = ranking::expects_directory(cmd);
        let wants_file = ranking::expects_file(cmd);
        let token_has_dir = token.contains('/');
        let dir_prefix = token_has_dir.then(|| &token[..token.rfind('/').unwrap() + 1]);
        let prefix_lower = (!prefix.is_empty()).then(|| prefix.to_lowercase());
        let middle_end = if trailing_space {
            parts.len()
        } else {
            parts.len().saturating_sub(1)
        };
        let middle = if parts.len() > 1 && 1 < middle_end {
            Some(parts[1..middle_end].join(" "))
        } else {
            None
        };

        let mut candidates = SmallVec::new();

        for entry in entries.iter() {
            if !prefix.is_empty() && !entry.name.starts_with(prefix) {
                let Some(prefix_lower) = prefix_lower.as_deref() else {
                    continue;
                };
                if !entry.name.to_lowercase().starts_with(prefix_lower) {
                    continue;
                }
            }

            if wants_dir && !entry.is_dir {
                continue;
            }

            let path_part = if let Some(dir_prefix) = dir_prefix {
                format!("{dir_prefix}{}", entry.name)
            } else {
                entry.name.clone()
            };

            let suggestion_text = if cmd.is_empty() {
                path_part.clone()
            } else if let Some(middle) = &middle {
                format!("{cmd} {middle} {path_part}")
            } else {
                format!("{cmd} {path_part}")
            };

            let type_score = if (wants_dir && entry.is_dir) || (wants_file && !entry.is_dir) {
                1.0
            } else if !wants_dir && !wants_file {
                0.5
            } else {
                0.3
            };

            let prefix_score = if prefix.is_empty() {
                0.1
            } else {
                ranking::prefix_exactness(&entry.name, prefix)
            };

            let features = ranking::RankingFeatures {
                prefix_exactness: prefix_score,
                cwd_similarity: 0.0,
                path_exists: type_score,
                recency: 0.0,
                frequency: 0.0,
                last_command_transition: 0.0,
                success_bonus: 0.0,
            };

            candidates.push(SuggestionCandidate {
                text: suggestion_text,
                source: "path".into(),
                score: features.score(),
                kind: SuggestionKind::Path,
            });
        }

        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup_fs() -> (TempDir, Arc<Mutex<Store>>) {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("scripts")).unwrap();
        std::fs::write(tmp.path().join("readme.md"), "hello").unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        (tmp, store)
    }

    fn req(buffer: &str, cwd: &Path) -> SuggestionRequest {
        SuggestionRequest {
            request_id: 1,
            buffer: buffer.into(),
            cursor: buffer.len(),
            cwd: cwd.to_path_buf(),
            session_id: "test".into(),
            last_command: None,
            timestamp_ms: 10_000,
        }
    }

    #[test]
    fn test_supports_path_commands() {
        let (_tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, false, 256);

        assert!(!plugin.supports(&req("cd ", &PathBuf::from("/tmp"))));
        assert!(plugin.supports(&req("pushd ", &PathBuf::from("/tmp"))));
        assert!(plugin.supports(&req("vim ", &PathBuf::from("/tmp"))));
        assert!(plugin.supports(&req("./foo", &PathBuf::from("/tmp"))));
        assert!(!plugin.supports(&req("echo hello", &PathBuf::from("/tmp"))));
        assert!(!plugin.supports(&req("", &PathBuf::from("/tmp"))));
    }

    #[test]
    fn test_pushd_suggests_directories_only() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, false, 256);

        let r = req("pushd ", tmp.path());
        let results = plugin.suggest(&r);

        for c in &results {
            assert!(
                c.text.contains("src") || c.text.contains("scripts"),
                "unexpected suggestion: {}",
                c.text
            );
        }
        assert!(!results.iter().any(|c| c.text.contains("readme.md")));
        assert!(!results.iter().any(|c| c.text.contains("Cargo.toml")));
    }

    #[test]
    fn test_vim_suggests_files() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, false, 256);

        let r = req("vim r", tmp.path());
        let results = plugin.suggest(&r);

        assert!(results.iter().any(|c| c.text.contains("readme.md")));
    }

    #[test]
    fn test_hidden_files_excluded_by_default() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, false, 256);

        let r = req("pushd ", tmp.path());
        let results = plugin.suggest(&r);

        assert!(!results.iter().any(|c| c.text.contains(".hidden")));
    }

    #[test]
    fn test_hidden_files_included_when_enabled() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, true, 256);

        let r = req("pushd ", tmp.path());
        let results = plugin.suggest(&r);

        assert!(results.iter().any(|c| c.text.contains(".hidden")));
    }

    #[test]
    fn test_prefix_filtering() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(store, false, 256);

        let r = req("pushd s", tmp.path());
        let results = plugin.suggest(&r);

        assert!(results.len() >= 1);
        for c in &results {
            let path_part = c.text.strip_prefix("pushd ").unwrap_or(&c.text);
            assert!(path_part.starts_with('s'), "unexpected: {}", c.text);
        }
    }

    #[test]
    fn test_path_cache_is_populated() {
        let (tmp, store) = setup_fs();
        let plugin = PathPlugin::new(Arc::clone(&store), false, 256);

        let r = req("pushd ", tmp.path());
        let _ = plugin.suggest(&r);

        let s = store.lock().unwrap();
        let cached = s
            .get_path_cache(&tmp.path().to_string_lossy(), CACHE_TTL_MS, 10_000)
            .unwrap();
        assert!(cached.is_some());
    }
}
