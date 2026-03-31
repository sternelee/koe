# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Koe (声) is a zero-GUI macOS voice input tool. Press a hotkey, speak, and corrected text is pasted into your active input field. The app runs as a menu bar agent app with no windows.

## Commands

```bash
# Build everything (Rust core + Xcode app)
make build

# Build Rust core only
make build-rust

# Build Xcode app only
make build-xcode

# Build for Intel Mac
make build-x86_64

# Regenerate Xcode project after modifying KoeApp/project.yml
cd KoeApp && xcodegen generate

# Clean build artifacts
make clean

# Run the app (Debug build)
make run
```

## Architecture

The app consists of two layers communicating via C FFI:

- **Objective-C shell** (`KoeApp/`): macOS integration — hotkey detection (Fn key with 180ms tap/hold threshold), audio capture (AVFoundation), clipboard management, paste simulation (Cmd+V), menu bar UI, and SQLite usage statistics
- **Rust core** (`koe-core/`, `koe-asr/`): Network operations — ASR WebSocket streaming (Doubao/豆包, Qwen/通义), local ASR (MLX via Swift FFI, sherpa-onnx via CPU worker), LLM correction (OpenAI-compatible API), config management, transcript aggregation, model management
- **Swift KoeMLX package**: Bridges MLX inference (Qwen3-ASR) to Rust via C FFI for on-device ASR on Apple Silicon

The Rust core is compiled as a static library (`libkoe_core.a`) and linked into the Xcode project.

### FFI Boundary

`koe-core/src/ffi.rs` defines the C ABI surface. The ObjC shell registers `SPCallbacks` (function pointers) at startup, then drives sessions via `sp_begin_session` / `sp_feed_audio` / `sp_end_session`. Callbacks fire on Rust-managed threads; the ObjC side dispatches to the main thread as needed. Each session carries a `session_token` (u64) so the ObjC caller can discard events from superseded sessions.

### Key Modules

- **ASR**: WebSocket streaming with two-pass recognition (Interim → Definite → Final)
- **LLM**: Text correction with dictionary and interim revision history context
- **TranscriptAggregator**: Merges streaming results, tracks revision history
- **Session**: State machine managing Idle → Recording → Finalizing → Correcting → Paste flow

### Configuration

All config lives in `~/.koe/`:
- `config.yaml` — ASR/LLM credentials, hotkey, feedback sounds
- `dictionary.txt` — one term per line, used for ASR hotwords + LLM correction
- `system_prompt.txt` / `user_prompt.txt` — LLM correction prompts
- `history.db` — SQLite usage statistics

### State Machine States

Idle → HotkeyDecisionPending → ConnectingAsr → RecordingHold/RecordingToggle → FinalizingAsr → Correcting → PreparingPaste → Pasting → RestoringClipboard → Completed → Idle

Any state can transition to `Failed` on error. The `Failed` state is terminal for that session.

## Dependencies

- Xcode with command line tools
- [xcodegen](https://github.com/yonaskolb/XcodeGen) — regenerate Xcode project after modifying KoeApp/project.yml
- Rust toolchain (targets aarch64-apple-darwin only)

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/). Use `/ship` skill if available:
```bash
npx skills add missuo/ship
/ship
```

Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore
