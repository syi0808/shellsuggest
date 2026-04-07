# shellsuggest

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

> Your shell should know where you are.

shellsuggest provides smarter zsh autosuggestions, ranked by your current directory. It validates that suggested paths actually exist and runs as a long-lived Rust daemon so every suggestion arrives in microseconds.

[zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions) is a great plugin that brought fish-style suggestions to zsh. shellsuggest builds on that idea with directory-aware ranking, path validation, and a plugin-based daemon architecture. LLM-powered completions are planned and coming soon.

```
# zsh-autosuggestions: same suggestion everywhere
~/project $ cd src     # suggests "cd src/old-thing" (from last week, different repo)
~/dotfiles $ cd src    # suggests "cd src/old-thing" (same wrong suggestion)

# shellsuggest: knows your cwd
~/project $ cd src     # suggests "cd src/components" (you cd'd here yesterday, in this repo)
~/dotfiles $ cd src    # suggests "cd src/zsh" (different dir, different suggestion)
```

## Features

- **Directory-aware history**: suggestions ranked by what you've run in this directory, and `cd` is restricted to this directory's own history
- **Transition-aware ranking**: the last successful command biases the next suggestion, so `vim main.rs` can push `make test` above other `make` commands
- **`cd` cold-start assist**: when local `cd` history is empty, suggest direct child directories from the current workspace
- **History import**: on daemon start, imports your existing zsh history as a global fallback for prefixes that have no live match yet
- **Path validation**: file/path commands like `vim` only suggest entries that actually exist on disk
- **Multiple candidates**: cycle through suggestions inline with `Alt+n` / `Alt+p`
- **Fast**: ~5us lookups at 100k history entries, ~15us end-to-end roundtrips on Apple Silicon
- **Ghost text**: suggestions appear inline as dimmed text after your cursor, like fish shell's autosuggestions

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

Install the zsh plugin into `~/.zshrc` automatically:

```bash
shellsuggest init
```

Or add the snippet manually:

```zsh
eval "$(shellsuggest init zsh)"
```

## Usage

### Key Bindings

| Key | Action |
|---|---|
| `->` (right arrow) | Accept full suggestion |
| `Alt+f` | Accept one word |
| `Alt+n` | Cycle to the next suggestion |
| `Alt+p` | Cycle to the previous suggestion |
| `Ctrl+->` / `Alt+->` | Accept one word when your terminal sends those keys |
| `Esc` | Clear suggestion |

### Migration From zsh-autosuggestions

`shellsuggest init` already checks `~/.zshrc` and runs the common migration path automatically. To run the rewrite directly or preview it:

```bash
shellsuggest migrate zsh-autosuggestions          # rewrite ~/.zshrc
shellsuggest migrate zsh-autosuggestions --dry-run # preview only
```

What it does:

- Disables `zsh-autosuggestions` source/plugin-manager lines
- Removes `zsh-autosuggestions` from `plugins=(...)`
- Appends `eval "$(shellsuggest init zsh)"`
- Prints warnings for settings that still need manual review

The plugin also accepts common `zsh-autosuggestions` carry-overs: `ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE`, `ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE`, `ZSH_AUTOSUGGEST_HISTORY_IGNORE`, `ZSH_AUTOSUGGEST_MANUAL_REBIND`, and the `autosuggest-*` widget names.

### CLI

```bash
shellsuggest serve     # start daemon (auto-started by plugin)
shellsuggest query     # client mode (used by zsh coproc)
shellsuggest status    # show daemon state, feedback totals, cache stats, config
shellsuggest journal   # inspect command history
shellsuggest init [--dry-run]  # inspect/update ~/.zshrc
shellsuggest init zsh  # output raw zsh plugin source
shellsuggest migrate zsh-autosuggestions [--dry-run]  # rewrite ~/.zshrc
```

### Configuration

`~/.config/shellsuggest/config.toml`:

```toml
[daemon]
socket_path = "/tmp/shellsuggest-{uid}.sock"  # default

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

### How It Works

```
zsh plugin (.zsh)          pure zsh, renders ghost text
       |
       | stdin/stdout (compact line protocol)
       v
shellsuggest query         coproc, relays to daemon
       |
       | Unix domain socket
       v
shellsuggest serve         long-lived Rust daemon
                           4 plugins, SQLite journal, session candidate state
```

Four suggestion plugins run on every keystroke:

1. **history**: traditional prefix match (baseline)
2. **cwd_history**: same as history but filtered/boosted by current directory
3. **path**: reads the filesystem, suggests real files/directories
4. **cd_assist**: direct child directories for `cd` when local history has no match

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

### Performance

Benchmarked on Apple Silicon (M3 Pro) with `cargo bench --bench suggest`:

| Scenario | Time |
|---|---|
| Broker lookup, 10k journal rows | ~3.0us |
| Broker lookup, 100k journal rows | ~4.9us |
| Transition-aware broker lookup, 100k journal rows | ~8.2us |
| Daemon roundtrip, 100k journal rows | ~14.8us |
| Path plugin, 256-entry directory | ~49us |

## Contributing

Contributions are welcome. Please read the [Contributing Guide](CONTRIBUTING.md) before submitting a pull request.

## License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.

## Author

**Yein Sung**: [GitHub](https://github.com/syi0808)
