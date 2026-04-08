# shellsuggest Design Spec

cwd-aware, path-validated inline suggestion engine for zsh.

## Problem

`zsh-autosuggestions` does history prefix match without current working directory context. That makes `cd` and other path-sensitive suggestions noisy and often wrong across repositories.

shellsuggest fixes that by combining:

- cwd-aware history ranking
- path validation against the current filesystem
- a long-lived query coprocess so suggestion work stays hot

## Goals

- More accurate inline suggestions than `zsh-autosuggestions`
- Low-latency suggestions without per-keystroke process spawn
- A single binary plus a small zsh plugin

## Non-Goals

- Replacing zsh completion
- Shell-framework-specific integrations
- LLM-powered per-keystroke suggestions

## Architecture

### Components

```
+---------------------+
| zsh plugin (.zsh)   |  rendering + widgets + coproc management
| pure zsh            |
+----------+----------+
           | stdin/stdout (compact line protocol)
           v
+---------------------+
| shellsuggest query  |  long-lived Rust coprocess
| query runtime       |  broker, session state, journal writes
+----------+----------+
           |
           v
+---------------------+
| SQLite journal      |  command history, feedback, path cache,
| + aggregate tables  |  seeded history, transition stats
+---------------------+
```

### Command model

```
shellsuggest query    # query runtime used by zsh coproc
shellsuggest status   # runtime mode + metrics + config snapshot
shellsuggest journal  # DB inspection/debug
shellsuggest init zsh # output zsh plugin source
```

There is no shared daemon process. Each shell keeps its own `shellsuggest query` coprocess alive.

### Key principles

1. zsh does rendering and key handling only
2. suggestion logic lives in the query runtime
3. the query process stays resident for the lifetime of the shell session
4. stale responses are discarded by `request_id`
5. SQLite is configured for multi-process access with WAL and `busy_timeout`

## Runtime Flow

### Query lifecycle

1. zsh loads the plugin and starts `shellsuggest query` as a coprocess
2. the query runtime opens SQLite, runs migrations, and prewarms seeded history from `HISTFILE`
3. each keystroke sends a compact line-protocol request over stdio
4. the runtime merges plugin results, returns the best candidates, and stores per-session cycle state in memory

### Session state

Candidate cycling (`Alt+n` / `Alt+p`) is local to a shell session. Each query process owns:

- current request id bookkeeping
- candidate lists per session id
- selected candidate index for cycling

## Storage

Data lives at `~/.local/share/shellsuggest/journal.db`.

### Tables

- `command_journal`: successful commands with cwd and timestamps
- `suggestion_feedback`: accepted/rejected suggestions
- `path_cache`: cached shallow directory listings
- `command_stats`: aggregated global command history
- `cwd_command_stats`: aggregated cwd-local history
- `history_seed_stats`: conservative history imported from `HISTFILE`
- `transition_stats`: command-following-command counts

### Concurrency model

Multiple shell sessions may run separate query processes concurrently. SQLite is opened with:

- `PRAGMA journal_mode = WAL`
- `PRAGMA synchronous = NORMAL`
- `busy_timeout = 1000ms`

That keeps the no-daemon model practical while preserving a shared journal file.

## Suggestion Plugins

### history

- Prefix matches against aggregated global history
- Used as the general fallback

### cwd_history

- Prefix matches scoped to the current cwd first
- May include parent cwd matches for non-`cd` commands
- Provides the strongest locality signal

### path

- Uses the filesystem and path cache
- Suggests only existing paths
- Bounded to shallow scans and a configurable entry cap

### cd_assist

- Fallback for builtin `cd`
- Suggests immediate child directories from the current cwd
- Disabled when cwd-local `cd` history already exists

## Ranking

```
score =
  0.30 * prefix_exactness
+ 0.25 * cwd_similarity
+ 0.20 * path_exists
+ 0.10 * recency
+ 0.05 * frequency
+ 0.05 * last_command_transition
+ 0.05 * success_bonus
```

Additional rules:

- dangerous commands (`rm`, `cp`, `mv`) require a higher score threshold
- builtin `cd` avoids pulling destination-directory history into the current directory

## zsh Frontend

The plugin is a single file: `plugin/shellsuggest.plugin.zsh`.

### Responsibilities

- start and supervise the `shellsuggest query` coprocess
- send query / cycle / feedback / record frames
- render ghost text via `POSTDISPLAY`
- keep highlight state in sync with accepted or cleared suggestions
- wrap existing widgets without breaking custom user widgets

### Widgets

| Widget | Binding | Action |
|---|---|---|
| `_shellsuggest_suggest` | `zle-line-pre-redraw` | request and render suggestion |
| `_shellsuggest_accept` | right arrow | accept full suggestion |
| `_shellsuggest_accept_word` | Ctrl+right / Alt+f | accept one word |
| `_shellsuggest_clear` | Esc | clear suggestion |
| `_shellsuggest_execute` | wraps `accept-line` | execute command and record result |

## Protocol

The plugin and query runtime use compact escaped tab-separated line frames over stdio.

Message families:

- suggest
- cycle
- feedback
- record
- suggestion
- ack
- error

The protocol is deliberately line-oriented so zsh can talk to the coprocess cheaply without JSON parsing on the hot path.

## Performance Targets

| Metric | Target |
|---|---|
| keypress to suggestion (p50) | < 2ms |
| keypress to suggestion (p95) | < 8ms |
| zsh startup overhead | < 20ms |
| query process cold start | < 80ms |
| per-plugin execution | < 10ms hard cap |

### Design decisions enforcing targets

1. long-lived query coprocess, not per-keystroke spawn
2. compact line protocol instead of heavyweight serialization
3. aggregate tables for history lookups
4. bounded path scanning and cache reuse
5. request-id-based stale response dropping

## Configuration

`~/.config/shellsuggest/config.toml`:

```toml
[path]
max_entries = 256
show_hidden = false

[cd]
mode = "builtin"                    # builtin | jump | disabled
fallback_mode = "current_dir_only"  # builtin mode only

[history]
seed_from_histfile = true
histfile_path = ""
seed_max_entries = 20000

[ui]
max_candidates = 5
```

## Testing

### Rust tests

- unit tests for ranking, plugins, protocol, config, and DB behavior
- snapshot tests for high-signal suggestion scenarios
- integration tests that launch `shellsuggest query` as a child process and exercise the real stdio protocol

### zsh/Ruby tests

- end-to-end widget behavior through tmux-backed terminal sessions
- compatibility coverage for migrated `zsh-autosuggestions` workflows
- acceptance, cycling, paste handling, vi mode, and wrapped-widget behavior

### Benchmarks

Criterion benchmarks cover:

- broker lookup at 10k rows
- broker lookup at 100k rows
- transition-aware broker lookup at 100k rows
- query protocol roundtrip at 100k rows
- path suggestions in a 256-entry directory

## Future Work

- richer status counters for per-process request rates and timing distributions
- optional background services only if a new capability truly requires them
- external plugin protocol for non-core providers
- data-driven ranking weight tuning from `suggestion_feedback`
