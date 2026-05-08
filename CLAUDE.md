# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Koe is a background-first macOS voice input tool. Press a hotkey, speak, and corrected text is pasted into the active app. It is a native macOS Agent App (no Dock icon) with an Objective-C shell, a Rust core library, and Swift packages for on-device inference.

## Build, Test, and Development Commands

### Full Build
```bash
make build          # Full Apple Silicon build (generate Xcode project, build Rust staticlib, build Xcode app, install CLI)
make build-lite     # Lite variant: cloud + Apple Speech only (excludes MLX and sherpa-onnx)
make build-x86_64   # Intel build (no MLX)
```

### Incremental / Development
```bash
make generate       # Regenerate Koe.xcodeproj from project.yml (xcodegen)
make build-rust     # Build Rust staticlib and CLI only
cargo test --workspace          # Run all Rust tests
cargo test -p koe-asr --test api_test    # Run a specific test file
cargo clippy --workspace        # Lint
cargo fmt                       # Format
make clean                      # Clean both cargo and Xcode build artifacts
```

### Run the App
```bash
make run            # Open the built Debug app
```
Or directly:
```bash
open ~/Library/Developer/Xcode/DerivedData/Koe-*/Build/Products/Release/Koe.app
```

## Workspace Structure

This is a Cargo workspace with three crates and a native macOS app:

| Component | Type | Purpose |
|---|---|---|
| `koe-core` | Rust staticlib + lib | Session orchestration, config, LLM correction, model management, C FFI |
| `koe-asr` | Rust lib | ASR provider implementations (Doubao, Qwen, Apple Speech, MLX, sherpa-onnx) and transcript aggregation |
| `koe-cli` | Rust bin | CLI for model management (`koe model list/pull/remove`) and manifest generation |
| `KoeApp/` | Objective-C / Xcode | macOS app shell: hotkey, audio capture, overlay, menu bar, paste, clipboard, permissions |
| `KoeApp/VirtualMicPlugin/` | Objective-C++ HAL plug-in | CoreAudio Audio Server Plug-in (`KoeVirtualMic.driver`) that exposes a virtual microphone fed by the translation pipeline's shared mmap output buffer. Embedded inside `Koe.app`. |
| `Packages/KoeMLX` | Swift package | Bridges MLX inference to Rust via C FFI for on-device ASR and LLM on Apple Silicon |
| `Packages/KoeAppleSpeech` | Swift package | Bridges Apple's SpeechAnalyzer to Rust via C FFI for zero-config on-device ASR (macOS 26+) |

## Architecture

### Language Boundary: Objective-C ↔ Rust via C FFI

- `koe-core/src/ffi.rs` defines the C ABI types (`SPSessionContext`, `SPCallbacks`, etc.) and callback dispatch helpers.
- `koe-core/build.rs` runs **cbindgen** to auto-generate `koe-core/target/koe_core.h` from the Rust FFI module. Do not edit the header manually.
- `KoeApp/Koe/Bridge/SPRustBridge.{h,m}` is the Objective-C wrapper that loads `libkoe_core.a`, registers callbacks, and exposes a high-level interface to the rest of the app.
- All callbacks are session-scoped and carry a monotonic `session_token`. The Objective-C side uses this token to discard stale events from superseded sessions.

### Session State Machine

A voice input session progresses through a strict state machine (`koe-core/src/session.rs`):

```
Idle → ConnectingAsr → RecordingHold / RecordingToggle → FinalizingAsr → Correcting → PreparingPaste → Pasting → RestoringClipboard → Completed
```

Invalid transitions return `KoeError::SessionInvalidState`. The `Session` struct tracks mode (hold vs toggle), frontmost app context, ASR text, and corrected text.

### ASR Pipeline

- **Cloud**: Doubao (WebSocket bidirectional streaming) and Qwen (WebSocket). First-pass `Interim` results stream to the overlay in real time; second-pass `Definite` results confirm segments with higher accuracy.
- **Local**: Apple Speech (SpeechAnalyzer, macOS 26+), MLX (Qwen3-ASR via Swift FFI, Apple Silicon), sherpa-onnx (CPU streaming zipformer).
- All providers emit the same `AsrEvent` enum (`Interim`, `Definite`, `Final`, `Error`, `Closed`).
- `TranscriptAggregator` (in `koe-asr`) merges results and tracks interim revision history, which is later passed to the LLM as context.
- `koe-core/src/asr_factory.rs` — single `build_asr_provider(cfg, dictionary)` that maps `cfg.asr.provider` to `(AsrConfig, Box<dyn AsrProvider>)`. Used by both the voice-input session and the translation engine. Adding a new provider only touches this module + `koe-asr`, not `lib.rs`.

### LLM Correction

- **OpenAI-compatible APIs**: Any cloud or self-hosted endpoint. The HTTP client is shared across sessions with HTTP/2 and connection pooling. A connection warm-up ping runs proactively to reduce first-request latency.
- **APFEL**: Local preset at `http://127.0.0.1:11434/v1`. Koe does not manage the APFEL lifecycle.
- **MLX**: Fully offline on Apple Silicon via the KoeMLX Swift package.
- Multiple LLM profiles are stored in config; `llm.active_profile` selects the endpoint.

### Real-time Translation Pipeline

A separate session type that runs alongside (not replacing) voice input. Mic audio enters, foreign-language audio exits via a virtual microphone for use in conferencing apps.

- `koe-core/src/translation/` — `TranslationEngine` plus `vad`, `mt`, `tts`, `output_bridge` submodules. The engine is async (tokio) and shuts down via `block_on(timeout(handle))` so the virtual-mic driver never sees a half-finished utterance.
- **Pipeline**: VAD-segmented mic PCM → ASR (via `asr_factory`, one fresh provider per utterance — ASR sessions are single-use) → MT (OpenAI-compatible chat completion) → TTS (ElevenLabs or MiniMax) → linear-interpolation resample (24 kHz → 48 kHz) → shared mmap ring buffer.
- **Output bridge**: `koe-core/src/translation/output_bridge.rs` and `KoeApp/VirtualMicPlugin/Sources/shared_buffer_protocol.h` define the same 48-byte header layout (magic `KOEV`, version, channel/sample-rate, capacity, **seqlock at offset 20**, write/read indices, last timestamp). The file lives at `/tmp/koe/virtual_mic_output.bin`. Reader (HAL plug-in) tolerates writer churn via a seqlock — odd sequence = writer mid-update, retry.
- **Buffer sizing**: `output_buffer_frames` defaults to **30 s @ 48 kHz**. TTS dumps an entire utterance at once; the HAL reader consumes at real-time playback rate. If the buffer is smaller than a full utterance, the writer laps the reader and only the tail plays. Do not lower this without changing the writer to chunk.
- **Virtual mic**: `KoeVirtualMic.driver` is an Audio Server Plug-in built as a separate Xcode bundle target and embedded inside `Koe.app/Contents/Library/...`. It mmaps the shared file read-only and renders frames into a CoreAudio device. Conferencing apps see a normal microphone.

### Feature Flags and Build Variants

Local ASR providers are controlled by Cargo features in `koe-core/Cargo.toml`:
- `mlx` — MLX local inference (Apple Silicon only)
- `apple-speech` — Apple SpeechAnalyzer (macOS 26+)
- `sherpa-onnx` — CPU streaming zipformer

| Xcode Scheme | Features | Notes |
|---|---|---|
| `Koe` | all defaults | Full build, Apple Silicon |
| `Koe-lite` | `apple-speech` only | Excludes MLX and sherpa-onnx (~78% smaller) |
| `Koe-x86` | `sherpa-onnx,apple-speech` | Intel Mac, no MLX |

The `KoeApp/project.yml` defines four targets — three app schemes plus the `KoeVirtualMic` HAL bundle (`.driver`) embedded into the main app. Each app scheme has a pre-build script that invokes `cargo build` with the correct `--features` flags and symlinks the staticlib into `target/release/`.

## Configuration and Runtime Files

All user configuration lives in `~/.koe/` and is auto-generated on first launch:
- `config.yaml` — main configuration (ASR provider, LLM profiles, hotkey, feedback)
- `dictionary.txt` — one term per line, used for ASR hotwords and LLM correction context
- `system_prompt.txt` / `user_prompt.txt` — LLM prompt customization
- `history.db` — SQLite usage statistics
- `models/` — local model manifests and downloaded weights

Config changes take effect automatically on the next hotkey press (hotkey changes within a few seconds). No restart is required.

## Commit and Release Conventions

All commits must follow [Conventional Commits](https://www.conventionalcommits.org/):
```
<type>(<scope>): <short summary>
```

Common scopes: `asr`, `llm`, `ui`, `config`, `setup`, `overlay`.

For release-worthy user-facing changes, update both `CHANGELOG.md` and `docs/update-feed.json`. PRs must still build with `make build` and should verify both hold-to-talk and tap-to-toggle modes.
