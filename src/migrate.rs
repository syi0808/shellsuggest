use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

const SHELLSUGGEST_INIT_LINE: &str = r#"eval "$(shellsuggest init zsh)""#;
const INIT_HEADER: &str = "# Added by `shellsuggest init`";
const MIGRATION_HEADER: &str = "# Added by `shellsuggest migrate zsh-autosuggestions`";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitAction {
    AlreadyInstalled,
    CreateZshrc,
    AppendInit,
    MigrateFromZshAutosuggestions,
}

impl InitAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AlreadyInstalled => "already-installed",
            Self::CreateZshrc => "create-zshrc",
            Self::AppendInit => "append-init",
            Self::MigrateFromZshAutosuggestions => "migrate-zsh-autosuggestions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub action: InitAction,
    pub changed: bool,
    pub warnings: Vec<String>,
    pub updated_contents: String,
    pub migration: Option<MigrationReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedInit {
    pub zshrc_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub report: InitReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub changed: bool,
    pub disabled_lines: usize,
    pub removed_plugin_tokens: usize,
    pub added_init: bool,
    pub warnings: Vec<String>,
    pub updated_contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub zshrc_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub report: MigrationReport,
}

pub fn default_zshrc_path() -> Option<PathBuf> {
    if let Ok(zdotdir) = std::env::var("ZDOTDIR") {
        let trimmed = zdotdir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(".zshrc"));
        }
    }

    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".zshrc"))
}

pub fn migrate_zsh_autosuggestions_file(path: &Path, dry_run: bool) -> Result<AppliedMigration> {
    if !path.exists() {
        bail!("zshrc not found at {}", path.display());
    }

    let original_contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read zshrc from {}", path.display()))?;
    let report = migrate_zsh_autosuggestions(&original_contents);

    if !report.changed {
        return Ok(AppliedMigration {
            zshrc_path: path.to_path_buf(),
            backup_path: None,
            report,
        });
    }

    let backup_path = if dry_run {
        None
    } else {
        let backup_path = backup_path_for(path)?;
        fs::write(&backup_path, original_contents)
            .with_context(|| format!("failed to write backup to {}", backup_path.display()))?;
        fs::write(path, &report.updated_contents)
            .with_context(|| format!("failed to update {}", path.display()))?;
        Some(backup_path)
    };

    Ok(AppliedMigration {
        zshrc_path: path.to_path_buf(),
        backup_path,
        report,
    })
}

pub fn init_zshrc_file(path: &Path, dry_run: bool) -> Result<AppliedInit> {
    let file_exists = path.exists();
    let original_contents = if file_exists {
        fs::read_to_string(path)
            .with_context(|| format!("failed to read zshrc from {}", path.display()))?
    } else {
        String::new()
    };
    let report = plan_zshrc_init(&original_contents, file_exists);

    if !report.changed {
        return Ok(AppliedInit {
            zshrc_path: path.to_path_buf(),
            backup_path: None,
            report,
        });
    }

    let backup_path = if dry_run || !file_exists {
        None
    } else {
        let backup_path = backup_path_for(path)?;
        fs::write(&backup_path, &original_contents)
            .with_context(|| format!("failed to write backup to {}", backup_path.display()))?;
        Some(backup_path)
    };

    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create parent directory for {}", path.display())
            })?;
        }
        fs::write(path, &report.updated_contents)
            .with_context(|| format!("failed to update {}", path.display()))?;
    }

    Ok(AppliedInit {
        zshrc_path: path.to_path_buf(),
        backup_path,
        report,
    })
}

pub fn migrate_zsh_autosuggestions(contents: &str) -> MigrationReport {
    let original_lines: Vec<&str> = contents.lines().collect();
    let mut updated_lines = Vec::with_capacity(original_lines.len() + 3);
    let mut warnings = BTreeSet::new();
    let mut disabled_lines = 0;
    let mut removed_plugin_tokens = 0;
    let mut saw_active_autosuggestions_ref = false;
    let mut has_shellsuggest_loader = false;

    for line in original_lines {
        let trimmed = line.trim_start();
        let is_comment = trimmed.starts_with('#');

        if !is_comment {
            if contains_shellsuggest_loader(trimmed) {
                has_shellsuggest_loader = true;
            }

            collect_warning(trimmed, &mut warnings);

            if let Some(updated) = remove_plugin_token_from_assignment(line, "zsh-autosuggestions")
            {
                removed_plugin_tokens += 1;
                saw_active_autosuggestions_ref = true;
                updated_lines.push(updated);
                continue;
            }

            if is_multiline_plugin_token(trimmed, "zsh-autosuggestions")
                || contains_active_autosuggestions_loader(trimmed)
            {
                disabled_lines += 1;
                saw_active_autosuggestions_ref = true;
                updated_lines.push(comment_out_line(line));
                continue;
            }
        }

        updated_lines.push(line.to_string());
    }

    let added_init = saw_active_autosuggestions_ref && !has_shellsuggest_loader;
    if added_init {
        if !updated_lines.is_empty()
            && !updated_lines
                .last()
                .is_some_and(|line| line.trim().is_empty())
        {
            updated_lines.push(String::new());
        }
        updated_lines.push(MIGRATION_HEADER.into());
        updated_lines.push(SHELLSUGGEST_INIT_LINE.into());
    }

    let updated_contents = normalize_lines(&updated_lines);
    let changed = updated_contents != normalize_text(contents);

    MigrationReport {
        changed,
        disabled_lines,
        removed_plugin_tokens,
        added_init,
        warnings: warnings.into_iter().collect(),
        updated_contents,
    }
}

fn plan_zshrc_init(contents: &str, file_exists: bool) -> InitReport {
    let migration = migrate_zsh_autosuggestions(contents);
    let has_active_autosuggestions =
        migration.disabled_lines > 0 || migration.removed_plugin_tokens > 0;

    if has_active_autosuggestions {
        return InitReport {
            action: InitAction::MigrateFromZshAutosuggestions,
            changed: migration.changed,
            warnings: migration.warnings.clone(),
            updated_contents: migration.updated_contents.clone(),
            migration: Some(migration),
        };
    }

    if has_shellsuggest_loader(contents) {
        return InitReport {
            action: InitAction::AlreadyInstalled,
            changed: false,
            warnings: migration.warnings,
            updated_contents: normalize_text(contents),
            migration: None,
        };
    }

    let updated_contents = append_shellsuggest_loader(contents, INIT_HEADER);
    InitReport {
        action: if file_exists {
            InitAction::AppendInit
        } else {
            InitAction::CreateZshrc
        },
        changed: updated_contents != normalize_text(contents),
        warnings: migration.warnings,
        updated_contents,
        migration: None,
    }
}

fn collect_warning(line: &str, warnings: &mut BTreeSet<String>) {
    if line.contains("ZSH_AUTOSUGGEST_STRATEGY") {
        warnings.insert(
            "review `ZSH_AUTOSUGGEST_STRATEGY`: shellsuggest uses its own ranking pipeline instead of zsh-autosuggestions strategies."
                .into(),
        );
    }

    if line.contains("ZSH_AUTOSUGGEST_COMPLETION_IGNORE") {
        warnings.insert(
            "remove `ZSH_AUTOSUGGEST_COMPLETION_IGNORE` manually: shellsuggest does not source completion-based suggestions."
                .into(),
        );
    }

    if line.contains("ZSH_AUTOSUGGEST_ACCEPT_WIDGETS")
        || line.contains("ZSH_AUTOSUGGEST_CLEAR_WIDGETS")
        || line.contains("ZSH_AUTOSUGGEST_EXECUTE_WIDGETS")
        || line.contains("ZSH_AUTOSUGGEST_PARTIAL_ACCEPT_WIDGETS")
        || line.contains("ZSH_AUTOSUGGEST_IGNORE_WIDGETS")
    {
        warnings.insert(
            "review custom `ZSH_AUTOSUGGEST_*_WIDGETS`: shellsuggest keeps its own widget wrapper set."
                .into(),
        );
    }
}

fn contains_shellsuggest_loader(line: &str) -> bool {
    line.contains("shellsuggest init zsh") || line.contains("shellsuggest.plugin.zsh")
}

fn has_shellsuggest_loader(contents: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#') && contains_shellsuggest_loader(trimmed)
    })
}

fn contains_active_autosuggestions_loader(line: &str) -> bool {
    line.contains("zsh-autosuggestions")
}

fn is_multiline_plugin_token(line: &str, token: &str) -> bool {
    let stripped = line.split('#').next().unwrap_or(line).trim();
    stripped == token
}

fn remove_plugin_token_from_assignment(line: &str, token: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("plugins=") {
        return None;
    }

    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }

    let before = &line[..open + 1];
    let inner = &line[open + 1..close];
    let after = &line[close..];
    let original_tokens: Vec<&str> = inner.split_whitespace().collect();
    let filtered_tokens: Vec<&str> = original_tokens
        .iter()
        .copied()
        .filter(|candidate| *candidate != token)
        .collect();

    if filtered_tokens.len() == original_tokens.len() {
        return None;
    }

    Some(format!("{before}{}{after}", filtered_tokens.join(" ")))
}

fn comment_out_line(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let body = line[indent_len..].trim_end();
    format!("{indent}# disabled by shellsuggest migrate: {body}")
}

fn append_shellsuggest_loader(contents: &str, header: &str) -> String {
    let mut updated_lines = contents.lines().map(String::from).collect::<Vec<_>>();
    if !updated_lines.is_empty()
        && !updated_lines
            .last()
            .is_some_and(|line| line.trim().is_empty())
    {
        updated_lines.push(String::new());
    }
    updated_lines.push(header.into());
    updated_lines.push(SHELLSUGGEST_INIT_LINE.into());
    normalize_lines(&updated_lines)
}

fn normalize_lines(lines: &[String]) -> String {
    let mut normalized = lines.join("\n");
    if !normalized.is_empty() {
        normalized.push('\n');
    }
    normalized
}

fn normalize_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut normalized = text.lines().collect::<Vec<_>>().join("\n");
    normalized.push('\n');
    normalized
}

fn backup_path_for(path: &Path) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before unix epoch")?
        .as_secs();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("zshrc path is missing a file name")?;

    Ok(path.with_file_name(format!("{file_name}.shellsuggest.bak-{timestamp}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn removes_zsh_autosuggestions_from_single_line_plugin_array() {
        let original = "plugins=(git zsh-autosuggestions fzf)\n";

        let report = migrate_zsh_autosuggestions(original);

        assert!(report.changed);
        assert_eq!(report.removed_plugin_tokens, 1);
        assert!(report.added_init);
        assert!(report.updated_contents.contains("plugins=(git fzf)"));
        assert!(report
            .updated_contents
            .contains(r#"eval "$(shellsuggest init zsh)""#));
    }

    #[test]
    fn comments_out_manual_source_lines() {
        let original = "source ~/.zsh/zsh-autosuggestions/zsh-autosuggestions.zsh\n";

        let report = migrate_zsh_autosuggestions(original);

        assert!(report.changed);
        assert_eq!(report.disabled_lines, 1);
        assert!(report.updated_contents.contains(
            "# disabled by shellsuggest migrate: source ~/.zsh/zsh-autosuggestions/zsh-autosuggestions.zsh"
        ));
    }

    #[test]
    fn does_not_duplicate_shellsuggest_loader() {
        let original = "\
plugins=(git zsh-autosuggestions)\n\
eval \"$(shellsuggest init zsh)\"\n";

        let report = migrate_zsh_autosuggestions(original);

        assert!(report.changed);
        assert!(!report.added_init);
        assert_eq!(
            report
                .updated_contents
                .matches("shellsuggest init zsh")
                .count(),
            1
        );
    }

    #[test]
    fn warns_about_settings_that_need_manual_review() {
        let original = "\
plugins=(git zsh-autosuggestions)\n\
ZSH_AUTOSUGGEST_STRATEGY=(history completion)\n\
ZSH_AUTOSUGGEST_COMPLETION_IGNORE='git *'\n\
ZSH_AUTOSUGGEST_ACCEPT_WIDGETS+=(forward-char)\n";

        let report = migrate_zsh_autosuggestions(original);

        assert_eq!(report.warnings.len(), 3);
    }

    #[test]
    fn init_appends_loader_when_no_existing_setup_is_present() {
        let original = "plugins=(git fzf)\n";

        let report = plan_zshrc_init(original, true);

        assert_eq!(report.action, InitAction::AppendInit);
        assert!(report.changed);
        assert!(report.updated_contents.contains(INIT_HEADER));
        assert!(report
            .updated_contents
            .contains(r#"eval "$(shellsuggest init zsh)""#));
    }

    #[test]
    fn init_preserves_raw_loader_contract_when_already_installed() {
        let original = "\
plugins=(git)\n\
eval \"$(shellsuggest init zsh)\"\n";

        let report = plan_zshrc_init(original, true);

        assert_eq!(report.action, InitAction::AlreadyInstalled);
        assert!(!report.changed);
        assert!(report.migration.is_none());
    }

    #[test]
    fn init_uses_migration_when_autosuggestions_are_active() {
        let original = "plugins=(git zsh-autosuggestions)\n";

        let report = plan_zshrc_init(original, true);

        assert_eq!(report.action, InitAction::MigrateFromZshAutosuggestions);
        assert!(report.changed);
        assert!(report.migration.is_some());
    }

    #[test]
    fn init_surfaces_manual_review_warnings_even_without_migration() {
        let original = "ZSH_AUTOSUGGEST_STRATEGY=(history completion)\n";

        let report = plan_zshrc_init(original, true);

        assert_eq!(report.action, InitAction::AppendInit);
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn writes_backup_and_updates_file() {
        let dir = TempDir::new().unwrap();
        let zshrc_path = dir.path().join(".zshrc");
        fs::write(&zshrc_path, "plugins=(git zsh-autosuggestions)\n").unwrap();

        let applied = migrate_zsh_autosuggestions_file(&zshrc_path, false).unwrap();

        assert!(applied.report.changed);
        let backup_path = applied.backup_path.expect("backup path");
        assert!(backup_path.exists());
        assert!(fs::read_to_string(&zshrc_path)
            .unwrap()
            .contains("shellsuggest init zsh"));
    }

    #[test]
    fn init_creates_missing_zshrc_without_backup() {
        let dir = TempDir::new().unwrap();
        let zshrc_path = dir.path().join(".zshrc");

        let applied = init_zshrc_file(&zshrc_path, false).unwrap();

        assert_eq!(applied.report.action, InitAction::CreateZshrc);
        assert!(applied.report.changed);
        assert!(applied.backup_path.is_none());
        assert!(fs::read_to_string(&zshrc_path)
            .unwrap()
            .contains("shellsuggest init zsh"));
    }
}
