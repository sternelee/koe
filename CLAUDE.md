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
make build-rust     # Build Rust staticlib + koe-cli
cargo test --workspace          # Run all Rust tests
cargo test -p koe-asr --test api_test    # Run a specific test file
cargo test -p koe-asr --test transcript_test    # Transcript aggregator integration tests
cargo test --manifest-path koe-core/Cargo.toml  # Test core (may need macOS SDK headers)
cargo clippy --workspace        # Lint
cargo fmt                       # Format
make clean                      # Clean both cargo and Xcode build artifacts
```

### Run the App
```bash
make run            # Open the built Debug app
```

### Prerequisites
- Xcode 16+, cmake (`brew install cmake`), xcodegen (`brew install xcodegen`)
- Rust toolchain with `aarch64-apple-darwin` target
- macOS 14.0+ deployment target (set in `.cargo/config.toml`)

### CI
Defined in `.github/workflows/release.yml` — runs on `macos-26`, tests koe-core then builds arm64/Xcode. On tag push `v*`, packages a DMG and uploads as a GitHub release.

## Workspace Structure

Three Rust crates + one Xcode project + three Swift packages:

| Component | Type | Purpose |
|---|---|---|
| `koe-core` | Rust staticlib + lib | Session orchestration, config, LLM correction, model management, C FFI, translation engine |
| `koe-asr` | Rust lib | ASR provider implementations and transcript aggregation |
| `koe-cli` | Rust bin | CLI for model management (`koe model list/pull/remove`) and manifest generation |
| `KoeApp/` | Objective-C / Xcode | macOS app shell: hotkey, audio capture, overlay, menu bar, paste, clipboard, permissions |
| `KoeApp/VirtualMicPlugin/` | Obj-C++ HAL plug-in | CoreAudio Audio Server Plug-in (`KoeVirtualMic.driver`) for virtual microphone in translation mode |
| `Packages/KoeMLX` | Swift package | Bridges MLX inference to Rust via C FFI for on-device ASR and LLM |
| `Packages/KoeAppleSpeech` | Swift package | Bridges Apple's SpeechAnalyzer to Rust via C FFI for zero-config on-device ASR (macOS 26+) |
| `Packages/KoeAppleTranslation` | Swift package | Bridges Apple's translation framework for on-device MT |

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

- **`AsrProvider` trait** (`koe-asr/src/provider.rs`): `connect`, `send_audio`, `finish_input`, `next_event`, `close` — all async.
- **`AsrEvent` enum** (`koe-asr/src/event.rs`): `Connected`, `Interim`, `Definite`, `Final`, `Error`, `Closed`.
- **Cloud**: Doubao (WebSocket bidirectional streaming), Doubao IME (free, no API key), Qwen (WebSocket). First-pass `Interim` results stream to the overlay; second-pass `Definite` results confirm segments with higher accuracy; `Final` ends the session.
- **Local**: Apple Speech (SpeechAnalyzer, macOS 26+), MLX (Qwen3-ASR via Swift FFI, Apple Silicon), sherpa-onnx (CPU streaming zipformer), whisper-rs.
- **`TranscriptAggregator`** (in `koe-asr`) merges Interim → Definite → Final events and tracks interim revision history for the LLM.

### ASR Factory

`koe-core/src/asr_factory.rs` — single `build_asr_provider(cfg, dictionary)` that maps `cfg.asr.provider` to `(AsrConfig, Box<dyn AsrProvider>)`. Used by both the voice-input session and the translation engine. Adding a new provider only touches this module + `koe-asr`.

### LLM Correction

- **`LlmProvider` trait** (`koe-core/src/llm/mod.rs`): single `correct(&self, request: &CorrectionRequest) -> Result<String>`.
- **OpenAI-compatible APIs**: Any cloud or self-hosted endpoint. HTTP client shared across sessions with HTTP/2 and connection pooling. Connection warm-up ping runs proactively to reduce first-request latency.
- **APFEL**: Local preset at `http://127.0.0.1:11434/v1`. Koe does not manage the APFEL lifecycle.
- **MLX**: Fully offline on Apple Silicon via the KoeMLX Swift package.
- Multiple LLM profiles stored in config; `llm.active_profile` selects the endpoint.

### Real-time Translation Pipeline

A separate session type that runs alongside (not replacing) voice input. Mic audio enters, foreign-language audio exits via a virtual microphone for conferencing apps.

- `koe-core/src/translation/` — `engine`, `config`, `vad`, `mt`, `tts`, `output_bridge`, `gemini_live`, `kitten`, `local_mt` submodules.
- **Pipeline**: VAD-segmented mic PCM → ASR (via `asr_factory`, one fresh provider per utterance — ASR sessions are single-use) → MT (OpenAI-compatible, local ONNX, or Apple Translation Framework) → TTS (ElevenLabs, MiniMax, Kokoro ONNX, Supertonic ONNX, Kitten ONNX) → linear-interpolation resample (24 kHz → 48 kHz) → shared mmap ring buffer. There is also a Gemini Live Translate API path that bypasses the VAD→ASR→MT→TTS pipeline.
- **Output bridge**: `koe-core/src/translation/output_bridge.rs` and `KoeApp/VirtualMicPlugin/Sources/shared_buffer_protocol.h` define the same 48-byte header layout (magic `KOEV`, version, channel/sample-rate, capacity, **seqlock at offset 20**, write/read indices, last timestamp). The file lives at `/tmp/koe/virtual_mic_output.bin`. The HAL plug-in reader tolerates writer churn via a seqlock — odd sequence numbers mean the writer is mid-update.
- **Buffer sizing**: `output_buffer_frames` defaults to 30 s @ 48 kHz. TTS dumps an entire utterance at once; the HAL reader consumes at real-time playback rate. If the buffer is smaller than a full utterance, the writer laps the reader and only the tail plays.
- **Virtual mic**: `KoeVirtualMic.driver` is an Audio Server Plug-in built as a separate Xcode bundle target and embedded inside `Koe.app/Contents/Library/...`. It mmaps the shared file read-only and renders frames into a CoreAudio device.

### Feature Flags and Build Variants

Local ASR/LLM/translation providers are controlled by Cargo features in `koe-core/Cargo.toml`:
- `mlx` — MLX local inference (Apple Silicon only)
- `apple-speech` — Apple SpeechAnalyzer (macOS 26+)
- `sherpa-onnx` — CPU streaming zipformer for ASR, and Kokoro/Supertonic ONNX for TTS
- `local-mt` — Local ONNX MT models (opus-mt)
- `kitten-onnx` — Kitten TTS ONNX runtime (requires ndarray, ort, misaki-rs)

| Xcode Scheme | Features | Notes |
|---|---|---|
| `Koe` | all defaults | Full build, Apple Silicon |
| `Koe-lite` | `apple-speech` only | Excludes MLX and sherpa-onnx (~78% smaller) |
| `Koe-x86` | `sherpa-onnx,apple-speech` | Intel Mac, no MLX |

The `KoeApp/project.yml` defines three app schemes plus the `KoeVirtualMic` HAL bundle. Each app scheme has a pre-build script that invokes `cargo build` with the correct `--features` flags and symlinks the staticlib into `target/release/`.

## Configuration and Runtime Files

All user configuration lives in `~/.koe/` and is auto-generated on first launch:
- `config.yaml` — main configuration (ASR provider, LLM profiles, hotkey, overlay, feedback, translation, prompt templates)
- `dictionary.txt` — one term per line, used for ASR hotwords and LLM correction context
- `system_prompt.txt` / `user_prompt.txt` — LLM prompt customization
- `history.db` — SQLite usage statistics
- `models/` — local model manifests and downloaded weights

Config changes take effect automatically on the next hotkey press (hotkey changes within a few seconds). No restart is required.

## Xcode Project Structure (KoeApp/Koe/)

| Group | Files | Purpose |
|---|---|---|
| `Accessibility/` | `SPAccessibilityManager.h/m` | Accessibility permission checks and AX API calls |
| `AppDelegate/` | `SPAppDelegate.h/m` | App lifecycle, menu bar, setup wizard orchestration |
| `Audio/` | `SPAudioCaptureManager.h/m`, `SPAudioDeviceManager.h/m`, `SPSystemAudioCaptureManager.h/m` | Mic audio capture, device enumeration, system audio capture |
| `Bridge/` | `SPRustBridge.h/m` | Objective-C ↔ Rust FFI bridge wrapping `libkoe_core.a` |
| `Clipboard/` | `SPClipboardManager.h/m` | Clipboard save/restore for paste automation |
| `Feedback/` | `SPFeedbackManager.h/m` | Haptic/audio feedback |
| `History/` | `SPHistoryManager.h/m` | Usage history display |
| `Hotkey/` | `SPHotkeyMonitor.h/m` | Global hotkey registration and event handling |
| `Localization/` | Localization helpers | Multi-language support |
| `Overlay/` | `SPOverlayManager.h/m`, `SPOverlayViewController.h/m`, etc. | Floating status pill showing interim/corrected text |
| `Paste/` | `SPPasteManager.h/m` | Auto-paste into frontmost app via Accessibility API |
| `Permissions/` | `SPPermissionsManager.h/m` | Mic, accessibility, screen recording permission prompts |
| `SetupWizard/` | Wizard VCs | First-launch setup flow |
| `StatusBar/` | Status bar controller | Menu bar item and status pill |
| `Update/` | `SPUpdateManager.h/m` | App update check against `docs/update-feed.json` |
| `VirtualMic/` | `SPVirtualMicManager.h/m` | Translation mode virtual mic start/stop |

## Test Patterns

- **Rust unit tests**: `#[test]` for sync tests (config parsing, transcript aggregation, state machine transitions).
- **Rust async tests**: `#[tokio::test]` for integration tests (ASR provider connections, audio streaming).
- **Test files**: `koe-asr/tests/api_test.rs` and `koe-asr/tests/transcript_test.rs` (integration tests). Inline `#[cfg(test)] mod tests` in source files.
- **Xcode tests**: `KoeApp/Koe/AppDelegate/SPAppDelegateLogicTests.m` and `KoeApp/Koe/Hotkey/SPHotkeyMonitor.m` (Objective-C XCTests).
- Run Rust tests with `cargo test --workspace` or target a specific test with `cargo test -p koe-asr --test transcript_test`.

## Commit and Release Conventions

All commits must follow [Conventional Commits](https://www.conventionalcommits.org/):
```
<type>(<scope>): <short summary>
```

Common scopes: `asr`, `llm`, `ui`, `config`, `setup`, `overlay`, `translation`, `models`.

For release-worthy user-facing changes, update both `CHANGELOG.md` and `docs/update-feed.json`.
