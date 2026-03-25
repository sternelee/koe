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

# Clean build artifacts
make clean

# Run the app (Debug build)
make run
```

## Architecture

The app consists of two layers communicating via C FFI:

- **Objective-C shell** (`KoeApp/`): macOS integration — hotkey detection (Fn key with 180ms tap/hold threshold), audio capture (AVFoundation), clipboard management, paste simulation (Cmd+V), menu bar UI, and SQLite usage statistics
- **Rust core** (`koe-core/`, `koe-asr/`): Network operations — ASR 2.0 WebSocket streaming (Doubao/豆包), LLM correction (OpenAI-compatible API), config management, transcript aggregation

The Rust core is compiled as a static library (`libkoe_core.a`) and linked into the Xcode project.

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
