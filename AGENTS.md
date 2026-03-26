# AGENTS.md

This file provides guidance for AI agents working in the Koe codebase.

## Project Overview

Koe (声) is a zero-GUI macOS voice input tool. The app consists of:
- **Objective-C shell** (`KoeApp/`): macOS integration, hotkey detection, audio capture, clipboard management
- **Rust core** (`koe-core/`, `koe-asr/`): ASR WebSocket streaming, LLM correction, config management

## Build Commands

```bash
# Build everything (Rust core + Xcode app)
make build

# Build Rust core only
make build-rust

# Build Xcode app only (requires xcodegen: cd KoeApp && xcodegen)
make build-xcode

# Build for Intel Mac
make build-x86_64

# Clean all build artifacts
make clean

# Run the app (Debug build)
make run
```

## Rust Commands

```bash
# Build release for Apple Silicon
cargo build --manifest-path koe-core/Cargo.toml --release --target aarch64-apple-darwin

# Run all tests
cargo test

# Run tests for a specific package
cargo test --package koe-asr
cargo test --package koe-core

# Run a single test
cargo test --package koe-asr test_transcript_aggregator_interim

# Run tests matching a pattern
cargo test test_transcript

# Run clippy lints
cargo clippy --all-targets --all-features

# Check formatting
cargo fmt --check

# Auto-format code
cargo fmt
```

## Code Style

### Rust

- **Formatting**: Use `cargo fmt` (rustfmt default style)
- **Lints**: Run `cargo clippy` before committing; address all warnings
- **Comments**: Avoid unnecessary comments; code should be self-documenting
- **Imports**: Group by external crates → internal modules → parent modules
  ```rust
  use std::sync::{Arc, Mutex};
  use tokio::runtime::Runtime;
  use crate::config::Config;
  use crate::errors::{KoeError, Result};
  ```
- **Naming**:
  - Types/enums: `PascalCase` (e.g., `SessionState`, `KoeError`)
  - Functions/variables: `snake_case` (e.g., `load_config`, `final_text`)
  - Constants: `SCREAMING_SNAKE_CASE` (e.g., `PROTOCOL_VERSION`)
  - Acronyms: Keep lowercase after first letter (e.g., `app_key`, not `appKey` or `APP_KEY`)
- **Error Handling**:
  - Custom error enums implementing `std::error::Error` and `fmt::Display`
  - Use `Result<T>` type alias pattern: `pub type Result<T> = std::result::Result<T, KoeError>;`
  - Wrap errors with context: `Err(KoeError::Config(format!("read {}: {e}", path.display())))`
- **Derives**: Use derive macros generously for boilerplate reduction
  ```rust
  #[derive(Debug, Deserialize, Clone)]
  pub struct Config { ... }
  ```
- **Async**: Use `tokio` runtime; prefer `async/await` over `.then()` chains
- **FFI**: Mark exported functions with `#[no_mangle]` and `pub extern "C"`

### Objective-C

- **Prefix**: All classes use `SP` prefix (e.g., `SPRustBridge`, `SPStatusBarManager`)
- **Naming**: Follow Apple conventions (`initWith...`, `did...`, `will...`)
- **Memory**: Use ARC; weak delegates to avoid retain cycles
- **Threading**: Dispatch to main queue for UI updates from callbacks

## Architecture Notes

### FFI Bridge

The Rust core exposes a C FFI interface in `koe-core/src/ffi.rs`. The Objective-C side calls these functions directly. All callbacks from Rust to Obj-C go through registered callback pointers.

### State Machine

Session states: `Idle → HotkeyDecisionPending → ConnectingAsr → RecordingHold/RecordingToggle → FinalizingAsr → Correcting → PreparingPaste → Pasting → RestoringClipboard → Completed → Idle`

### ASR Pipeline

1. Audio streams to Doubao ASR via WebSocket (binary gzip protocol)
2. Two-pass recognition: Interim → Definite → Final
3. `TranscriptAggregator` merges results and tracks revision history
4. Final transcript + interim history + dictionary sent to LLM

### Configuration

All config lives in `~/.koe/`:
- `config.yaml` — ASR/LLM credentials, hotkey, feedback sounds
- `dictionary.txt` — one term per line
- `system_prompt.txt` / `user_prompt.txt` — LLM correction prompts
- `history.db` — SQLite usage statistics

## File Structure

```
koe/
├── Cargo.toml              # Workspace root
├── Makefile                # Build orchestration
├── CLAUDE.md               # Human-focused guidance
├── koe-core/               # Rust core library
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # FFI entry points, session orchestration
│       ├── config.rs       # YAML config loading
│       ├── session.rs      # State machine
│       ├── errors.rs       # Error types
│       ├── dictionary.rs   # Dictionary loading
│       ├── prompt.rs       # LLM prompt rendering
│       ├── llm/            # LLM providers
│       └── ffi.rs          # C FFI bindings
├── koe-asr/                # ASR client library
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs          # Public API
│   │   ├── doubao.rs       # Doubao WebSocket provider
│   │   ├── config.rs       # ASR config
│   │   ├── error.rs        # ASR errors
│   │   ├── event.rs        # ASR event types
│   │   ├── transcript.rs   # TranscriptAggregator
│   │   └── provider.rs     # Provider trait
│   └── tests/
│       └── api_test.rs     # Integration tests
└── KoeApp/                 # Xcode/Objective-C app
    ├── project.yml         # XcodeGen config
    └── Koe/
        ├── Bridge/         # FFI bridge to Rust
        ├── Hotkey/        # Global hotkey monitoring
        ├── Audio/         # AVAudioCapture
        ├── StatusBar/     # Menu bar UI
        └── ...
```

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>

Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore
```

Example: `feat(asr): add two-pass recognition support`

## Testing Strategy

- **Unit tests**: Place in `src/` next to the code with `#[cfg(test)] mod tests { ... }`
- **Integration tests**: Place in `tests/` directory
- **Test naming**: `test_<what_is_being_tested>`
- **Async tests**: Use `#[tokio::test]` macro

## Debugging Tips

- **Rust logs**: Use `log::info!`, `log::warn!`, `log::error!`, `log::debug!`
- **OS logs**: View with `Console.app` or `log show --predicate 'process == "Koe"'`
- **Xcode**: Open `KoeApp/Koe.xcodeproj` for Obj-C debugging
