# Contributing to shellsuggest

Thank you for your interest in contributing to shellsuggest. This guide explains how to report issues, suggest improvements, and submit code changes.

## Code of Conduct

Please be respectful and constructive in all interactions. We are committed to providing a welcoming and inclusive experience for everyone.

## How to Contribute

### Reporting Bugs

1. Search [existing issues](../../issues) to check if the bug has already been reported
2. If not, open a new issue with:
   - Steps to reproduce the bug
   - Expected behavior vs. actual behavior
   - Your environment (macOS version, zsh version, Rust version)
   - Terminal emulator you are using

### Suggesting Enhancements

1. Search [existing issues](../../issues) for similar suggestions
2. Open a new issue describing:
   - The problem or use case
   - Your proposed solution
   - Alternatives you considered

### Pull Requests

1. Fork the repository
2. Create a feature branch from `main` (`git checkout -b feature/your-feature`)
3. Make your changes
4. Ensure the project builds and tests pass
5. Write clear commit messages (see Style Guide below)
6. Push to your fork and open a pull request
7. Fill in the PR description explaining what changed and why

## Development Setup

```bash
git clone https://github.com/YOUR_USERNAME/shellsuggest.git
cd shellsuggest
cargo build --release
```

The built binary is at `target/release/shellsuggest`. To install it and activate the zsh plugin:

```bash
cargo install --path .
shellsuggest init
```

### E2E Tests

E2E tests require tmux and Ruby 3.2.0:

```bash
brew install tmux ruby
export PATH="/opt/homebrew/opt/ruby/bin:$PATH"
bundle config set --local path vendor/bundle
bundle install
```

## Style Guide

### Code Style

Follow the existing patterns in the codebase. Key conventions:

- Use `anyhow::Result` for error handling
- Use the `tracing` crate for logging, not `println!`
- Keep public APIs minimal: prefer `pub(crate)` over `pub` where possible

### Commit Messages

This project loosely follows [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` for new features
- `fix:` for bug fixes
- `perf:` for performance improvements
- `docs:` for documentation changes

Use the imperative mood: "Add feature" not "Added feature". Keep the first line under 72 characters.

## Testing

Run the test suite before submitting a pull request:

```bash
# Rust unit and integration tests
cargo test

# E2E tests (requires tmux + Ruby)
bundle exec rspec

# Benchmarks
cargo bench --bench suggest
```

Rust tests are located in `tests/`. E2E tests are in `spec/` and use RSpec with tmux-based terminal simulation.
