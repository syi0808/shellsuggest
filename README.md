# shellsuggest

cwd-aware inline suggestion engine for zsh.

Unlike [zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions) which suggests based on history prefix match alone, shellsuggest knows **where you are**. It ranks suggestions by your current directory, validates that suggested paths actually exist, and keeps a long-lived Rust query coprocess hot so the interactive path stays in the microsecond range on local benchmarks.

## Reality Check

- This is a vibe-coded tool.
- The MVP spent roughly 2 hours on design and 5+ hours on QA.
- The author runs it daily on two Macs.

## Why

```
# zsh-autosuggestions: same suggestion everywhere
~/project $ cd src     # suggests "cd src/old-thing" (from last week, different repo)
~/dotfiles $ cd src    # suggests "cd src/old-thing" (same wrong suggestion)

# shellsuggest: knows your cwd
~/project $ cd src     # suggests "cd src/components" (you cd'd here yesterday, in this repo)
~/dotfiles $ cd src    # suggests "cd src/zsh" (different dir, different suggestion)
```

## Features

- **CWD-aware history** - suggestions ranked by what you've run in this directory, and `cd` is restricted to this directory's own history
- **Transition-aware ranking** - the last successful command biases the next suggestion, so `vim main.rs` can push `make test` above other `make` commands
- **`cd` cold-start assist** - when local `cd` history is empty, suggest direct child directories from the current workspace
- **`HISTFILE` prewarm** - on query process start, import recent zsh history as a global fallback for prefixes that have no live journal match yet
- **Async coprocess runtime** - suggestion work stays in a hot Rust query process instead of paying per-keystroke process spawn overhead in zsh
- **Path validation** - file/path commands like `vim` only suggest entries that actually exist
- **Multiple candidates** - keep the best few suggestions in memory and cycle them inline with `Alt+j` / `Alt+k`
- **Fast** - Rust query engine with pre-aggregated SQLite summaries, ~4.7us broker lookups and ~5.9us query protocol roundtrips at 100k rows on current local benchmarks
- **Ghost text** - inline suggestion rendered via `POSTDISPLAY`, just like fish shell
- **Feedback-aware metrics** - accepted/cleared suggestions are recorded for status reporting and future tuning
- **Single binary** - `cargo install shellsuggest`, one line in `.zshrc`

## Getting Started

### Requirements

- Rust toolchain (stable)
- zsh
- macOS or Linux (requires Unix domain sockets)

### Install

**Homebrew** (macOS / Linux):

```bash
brew install syi0808/shellsuggest/shellsuggest
```

**Cargo** (requires Rust toolchain):

```bash
cargo install shellsuggest
```

**From source**:

```bash
git clone https://github.com/syi0808/shellsuggest.git
cd shellsuggest
cargo install --path .
```

### Setup

Install into `~/.zshrc` automatically:

```bash
shellsuggest init
```

If you want the raw shell snippet instead, add this manually:

```zsh
eval "$(shellsuggest init zsh)"
```

## Migration From zsh-autosuggestions

`shellsuggest init` already checks `~/.zshrc` and runs the common migration path automatically. If you want to run the rewrite directly or preview it:

```bash
shellsuggest migrate zsh-autosuggestions
```

What it does:

- disables `zsh-autosuggestions` source/plugin-manager lines
- removes `zsh-autosuggestions` from `plugins=(...)`
- appends `eval "$(shellsuggest init zsh)"`
- prints warnings for settings that still need manual review

Use `--dry-run` to preview the summary without writing the file:

```bash
shellsuggest migrate zsh-autosuggestions --dry-run
```

The plugin also accepts a few common `zsh-autosuggestions` carry-overs during migration:

- `ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE`
- `ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE`
- `ZSH_AUTOSUGGEST_HISTORY_IGNORE`
- `ZSH_AUTOSUGGEST_MANUAL_REBIND`
- `autosuggest-accept` / `autosuggest-clear` / `autosuggest-execute` / `autosuggest-fetch` / `autosuggest-enable` / `autosuggest-disable` / `autosuggest-toggle`

## Key Bindings

| Key | Action |
|---|---|
| `->` (right arrow) | Accept full suggestion |
| `Alt+f` | Accept one word |
| `Alt+j` | Cycle to the next suggestion |
| `Alt+k` | Cycle to the previous suggestion |
| `Alt+;` | Clear suggestion |

## Architecture

```
zsh plugin (.zsh)          pure zsh, renders ghost text, manages coproc
       |
       | stdin/stdout (compact line protocol)
       v
shellsuggest query         long-lived Rust coprocess per shell
                           broker, session candidate state, journal writes
       |
       v
SQLite journal             shared across shells via WAL
                           history, feedback, path cache, aggregate stats
```

Important architectural points:

- There is no shared daemon. Each interactive zsh session keeps one `shellsuggest query` coprocess alive.
- Candidate cycling state is local to that shell session.
- Command history and feedback are shared across shells through `~/.local/share/shellsuggest/journal.db`.
- SQLite is opened in WAL mode with a busy timeout so multiple shell sessions can reuse the same journal safely.

Four suggestion plugins participate on each query:

1. **history** - traditional prefix match (baseline)
2. **cwd_history** - same as history but filtered/boosted by current directory
3. **path** - reads the filesystem, suggests real files/directories
4. **cd_assist** - direct child directories for `cd` when local history has no match

Results are merged and ranked:

```
score =
  0.30 * prefix_exactness
+ 0.25 * cwd_similarity
+ 0.20 * path_exists
+ 0.10 * recency
+ 0.05 * frequency
+ 0.05 * command_transition
+ 0.05 * success_bonus
```

Dangerous commands (`rm`, `mv`, `cp`) require a higher confidence score to be suggested.

## CLI

```bash
shellsuggest query     # query runtime used by the zsh coproc
shellsuggest status    # show runtime model, feedback totals, cache stats, config
shellsuggest journal   # inspect command history
shellsuggest init [--zshrc PATH] [--dry-run]  # inspect/update ~/.zshrc
shellsuggest init zsh  # output raw zsh plugin source
shellsuggest migrate zsh-autosuggestions [--zshrc PATH] [--dry-run]  # rewrite ~/.zshrc
```

`shellsuggest query` is normally started by the plugin rather than run by hand.

## Configuration

`~/.config/shellsuggest/config.toml`:

```toml
[path]
max_entries = 256      # max directory entries to scan
show_hidden = false    # suggest hidden files/dirs

[cd]
fallback_mode = "current_dir_only"

[history]
seed_from_histfile = true
histfile_path = ""      # default: $HISTFILE or ~/.zsh_history
seed_max_entries = 20000

[ui]
max_candidates = 5
```

Data is stored at `~/.local/share/shellsuggest/journal.db`.

`shellsuggest query`, `status`, and `journal` all load the same config file. Invalid TOML causes those commands to exit with an error.

All shell sessions share that same SQLite file. The runtime uses WAL mode and a busy timeout so separate `query` processes can read and write without needing a daemon.

`HISTFILE` prewarm is intentionally conservative:

- it only affects the global `history` fallback
- it does not populate cwd-specific history, transitions, exit codes, or durations
- live journal matches always win over seeded history for the same prefix

## Performance

Benchmarked on Apple Silicon with `cargo bench --bench suggest`:

| Scenario | Time |
|---|---|
| Broker lookup, 10k journal rows | ~3.1us |
| Broker lookup, 100k journal rows | ~4.7us |
| Transition-aware broker lookup, 100k journal rows | ~7.9us |
| Query protocol roundtrip, 100k journal rows | ~5.9us |
| Path plugin, 256-entry directory | ~51us |

Notes:

- `Broker lookup` measures `Broker::suggest()` directly with an in-memory SQLite store.
- `Transition-aware broker lookup` measures `Broker::suggest()` when transition scoring is active.
- `Query protocol roundtrip` measures compact line encode/parse plus runtime handling on a reused query runtime.
- `Path plugin` measures a real `pushd` directory suggestion over a 256-entry directory.
- These numbers are core-engine benchmarks. Real interactive latency also includes zsh widget/rendering overhead.
- All benchmark numbers in the table come from `cargo bench --bench suggest` on the machine listed below.

Local process measurements on the author's machine, taken against `target/release/shellsuggest query` over a 10,000-request run:

- ~0.90us CPU time per query (median)
- ~4.5-4.8 MB RSS for the long-lived query process
- CPU time was measured from the query process's own cumulative user+system CPU counters before and after the run.
- RSS was read from the same process during the run; these numbers describe the long-lived query process itself, not the surrounding shell, terminal, or tmux session.
- The run used a temporary isolated `XDG_DATA_HOME`/`XDG_CONFIG_HOME`, preloaded a small warmup history set, then sent 10,000 real line-protocol requests over stdio.

Benchmark environment:

- MacBook Pro (`Mac15,6`)
- Apple M3 Pro, 11 CPU cores
- 18 GB memory
- macOS 26.2 (`25C56`), `arm64`
- `rustc 1.93.1 (01f6ddf75 2026-02-11)`

## Testing

shellsuggest leans hard on automated coverage for a shell plugin:

- 47 tmux/RSpec end-to-end examples drive a real `zsh -f` session
- 6 of those E2E scenarios are ported or adapted from `zsh-autosuggestions`
- 41 additional E2E examples cover shellsuggest-specific behavior
- Another 104 Rust tests cover ranking, protocol, snapshots, DB behavior, and child-process integration

## Development

```bash
# Build
cargo build --release

# Run all Rust tests
cargo test

# Run E2E tests (requires tmux + ruby; expects the release binary)
brew install tmux ruby
export PATH="/opt/homebrew/opt/ruby/bin:$PATH"
bundle config set --local path vendor/bundle
bundle install
bundle exec rspec

# Benchmarks
cargo bench --bench suggest
```

## License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.
