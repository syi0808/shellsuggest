use std::path::PathBuf;

use clap::{Parser, Subcommand};
use shellsuggest::config::Config;

#[derive(Parser)]
#[command(
    name = "shellsuggest",
    version,
    about = "cwd-aware inline suggestion engine for zsh"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Query mode (zsh coproc engine)
    Query,
    /// Show runtime status and perf counters
    Status,
    /// Inspect the command journal
    Journal,
    /// Install into ~/.zshrc, or output raw shell source with `init zsh`
    Init {
        /// Shell name (currently only "zsh") for raw plugin source output
        shell: Option<String>,
        /// Path to the zshrc file to inspect or update
        #[arg(long)]
        zshrc: Option<PathBuf>,
        /// Preview changes without writing the file
        #[arg(long)]
        dry_run: bool,
    },
    /// Rewrite ~/.zshrc to migrate from zsh-autosuggestions
    Migrate {
        /// Migration source (currently only "zsh-autosuggestions")
        source: String,
        /// Path to the zshrc file to update
        #[arg(long)]
        zshrc: Option<PathBuf>,
        /// Preview changes without writing the file
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> anyhow::Result<()> {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }

    let cli = Cli::parse();
    match cli.command {
        Commands::Query => {
            let config = Config::load()?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(shellsuggest::client::run(&config))?;
        }
        Commands::Status => {
            let config = Config::load()?;
            println!("runtime: per-shell query process (no shared daemon)");

            let db_path = shellsuggest::runtime::default_db_path();
            if db_path.exists() {
                println!("db: {}", db_path.display());
                let store = shellsuggest::db::store::Store::open(db_path.to_str().unwrap())?;
                let journal_rows = store.journal_count()?;
                let (accepted, rejected) = store.feedback_counts()?;
                let total_feedback = accepted + rejected;
                let acceptance_rate = if total_feedback == 0 {
                    0.0
                } else {
                    accepted as f64 / total_feedback as f64 * 100.0
                };
                let accepted_by_source = store.accepted_feedback_by_source()?;
                let path_cache_rows = store.path_cache_count()?;

                println!("journal_rows: {}", journal_rows);
                println!("feedback.accepted: {}", accepted);
                println!("feedback.rejected: {}", rejected);
                println!("feedback.acceptance_rate: {:.1}%", acceptance_rate);
                if accepted_by_source.is_empty() {
                    println!("feedback.by_source: none");
                } else {
                    let summary = accepted_by_source
                        .into_iter()
                        .map(|(source, count)| format!("{source}={count}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("feedback.by_source: {}", summary);
                }
                println!("path_cache_rows: {}", path_cache_rows);
                println!("history_seed_rows: {}", store.seeded_command_count()?);
            } else {
                println!("db: not found");
                println!("journal_rows: 0");
                println!("feedback.accepted: 0");
                println!("feedback.rejected: 0");
                println!("feedback.acceptance_rate: 0.0%");
                println!("feedback.by_source: none");
                println!("path_cache_rows: 0");
                println!("history_seed_rows: 0");
            }
            println!("{}", config);
        }
        Commands::Journal => {
            let _config = Config::load()?;
            let db_path = shellsuggest::runtime::default_db_path();
            if !db_path.exists() {
                println!("no journal found at {}", db_path.display());
                return Ok(());
            }
            let store = shellsuggest::db::store::Store::open(db_path.to_str().unwrap())?;
            let entries = store.recent_entries(20)?;
            if entries.is_empty() {
                println!("journal is empty");
            } else {
                for entry in entries {
                    println!(
                        "[{}] {} (cwd: {}, exit: {})",
                        entry.timestamp,
                        entry.command_line,
                        entry.cwd,
                        entry
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".into()),
                    );
                }
            }
        }
        Commands::Init {
            shell,
            zshrc,
            dry_run,
        } => match shell.as_deref() {
            Some("zsh") => {
                if zshrc.is_some() || dry_run {
                    anyhow::bail!(
                        "`--zshrc` and `--dry-run` are only supported by `shellsuggest init` without a shell argument."
                    );
                }
                print!("{}", include_str!("../plugin/shellsuggest.plugin.zsh"));
            }
            Some(other) => {
                anyhow::bail!("unsupported shell: {other}. only 'zsh' is supported.");
            }
            None => {
                let zshrc_path = zshrc
                    .or_else(shellsuggest::migrate::default_zshrc_path)
                    .ok_or_else(|| anyhow::anyhow!("could not determine a default zshrc path"))?;
                let applied = shellsuggest::migrate::init_zshrc_file(&zshrc_path, dry_run)?;

                println!("zshrc: {}", applied.zshrc_path.display());
                println!("mode: {}", if dry_run { "dry-run" } else { "applied" });
                println!("action: {}", applied.report.action.as_str());

                if !applied.report.changed {
                    println!("changes: none");
                } else {
                    if let Some(migration) = &applied.report.migration {
                        println!("disabled_lines: {}", migration.disabled_lines);
                        println!("removed_plugin_tokens: {}", migration.removed_plugin_tokens);
                        println!("added_shellsuggest_init: {}", migration.added_init);
                    } else {
                        println!("added_shellsuggest_init: true");
                    }

                    if let Some(backup_path) = applied.backup_path {
                        println!("backup: {}", backup_path.display());
                    }
                }

                if applied.report.warnings.is_empty() {
                    println!("warnings: none");
                } else {
                    println!("warnings:");
                    for warning in applied.report.warnings {
                        println!("  - {warning}");
                    }
                }
            }
        },
        Commands::Migrate {
            source,
            zshrc,
            dry_run,
        } => {
            if source != "zsh-autosuggestions" {
                anyhow::bail!(
                    "unsupported migration source: {source}. only 'zsh-autosuggestions' is supported."
                );
            }

            let zshrc_path = zshrc
                .or_else(shellsuggest::migrate::default_zshrc_path)
                .ok_or_else(|| anyhow::anyhow!("could not determine a default zshrc path"))?;
            let applied =
                shellsuggest::migrate::migrate_zsh_autosuggestions_file(&zshrc_path, dry_run)?;

            println!("zshrc: {}", applied.zshrc_path.display());
            println!("mode: {}", if dry_run { "dry-run" } else { "applied" });

            if !applied.report.changed {
                println!("changes: none");
            } else {
                println!("disabled_lines: {}", applied.report.disabled_lines);
                println!(
                    "removed_plugin_tokens: {}",
                    applied.report.removed_plugin_tokens
                );
                println!("added_shellsuggest_init: {}", applied.report.added_init);

                if let Some(backup_path) = applied.backup_path {
                    println!("backup: {}", backup_path.display());
                }
            }

            if applied.report.warnings.is_empty() {
                println!("warnings: none");
            } else {
                println!("warnings:");
                for warning in applied.report.warnings {
                    println!("  - {warning}");
                }
            }
        }
    }
    Ok(())
}
