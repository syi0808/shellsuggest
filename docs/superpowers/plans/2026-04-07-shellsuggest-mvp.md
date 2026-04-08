# shellsuggest Current Implementation Notes

This file replaces the earlier daemon-based MVP plan.

The original step-by-step plan targeted a shared daemon plus socket relay. The current codebase no longer follows that design, so the old plan was more confusing than useful. This document now tracks the current implementation shape and the main maintenance checkpoints.

## Current Architecture

- `shellsuggest query` is the long-lived Rust coprocess used by the zsh plugin
- there is no shared daemon and no Unix socket transport
- the query runtime owns broker execution, per-session candidate cycling state, and journal writes
- all shell sessions share one SQLite journal file through WAL mode

## Active File Map

```text
src/
  client/mod.rs        # stdio loop for the query coprocess
  runtime/mod.rs       # query runtime, message handling, DB startup helpers
  daemon/broker.rs     # broker and plugin fan-out
  db/store.rs          # SQLite access, migrations, aggregate-table maintenance
  plugin/*.rs          # history, cwd_history, path, cd_assist providers
  protocol/mod.rs      # compact escaped line protocol
  main.rs              # query/status/journal/init/migrate CLI
plugin/
  shellsuggest.plugin.zsh
tests/
  golden.rs
  integration.rs
spec/
  *_spec.rb
benches/
  suggest.rs
```

## Maintenance Checklist

### Runtime

- keep `shellsuggest query` startup cheap
- preserve per-shell coprocess semantics
- avoid reintroducing per-keystroke subprocess spawn

### Storage

- keep aggregate tables in sync with journal writes
- preserve WAL + `busy_timeout` configuration for multi-process access
- keep `HISTFILE` seeding conservative so live journal data still wins

### Suggestions

- builtin `cd` must remain cwd-local by default
- path suggestions must stay bounded by `max_entries`

### Tests

- `cargo test` should cover protocol, ranking, plugins, DB, snapshots, and child-process integration
- `bundle exec rspec` should cover zsh widget and terminal-session behavior
- integration tests should exercise the real stdio protocol, not internal helper shortcuts

### Benchmarks

- broker lookup at 10k and 100k rows
- transition-aware broker lookup
- query protocol roundtrip
- path-plugin bounded directory scan

## When Updating Docs

- prefer README and the design spec for user-facing behavior
- treat this file as an engineer-facing architecture snapshot
- if architecture changes again, rewrite this document instead of layering historical notes on top
