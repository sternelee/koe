# AGENTS.md

Compact guidance for AI coding agents working in the Koe repository.

## Project

Koe is a native macOS voice-input app (no Dock icon) with an Objective-C shell, Rust core library, and Swift packages for on-device inference. Press a hotkey, speak, and corrected text is pasted into the active app.

## Build & Development

Full build requires **xcodegen** (`brew install xcodegen`) and **cmake** (`brew install cmake`, required by `audiopus_sys`).

```bash
make build          # Apple Silicon: generate Xcode project, build Rust staticlib + CLI, build app, install CLI
make build-lite     # Cloud + Apple Speech only (excludes MLX and sherpa-onnx)
make build-x86_64   # Intel Mac (no MLX)
make generate       # Regenerate Koe.xcodeproj from KoeApp/project.yml
make build-rust     # Build Rust staticlib and CLI only (no Xcode app)
make run            # Open built Debug app
make clean          # Clean cargo + Xcode artifacts
```

Run a single Rust test file:
```bash
cargo test -p koe-asr --test api_test
```

Lint and format:
```bash
cargo clippy --workspace
cargo fmt
```

## Workspace Structure

| Path | Role |
|---|---|
| `koe-core/` | Rust staticlib + lib. Session orchestration, config, LLM correction, model management, C FFI |
| `koe-asr/` | Rust lib. ASR providers (Doubao, Qwen, Apple Speech, MLX, sherpa-onnx) + transcript aggregation |
| `koe-cli/` | Rust bin. Model management CLI (`koe model list/pull/remove`) |
| `KoeApp/` | Objective-C Xcode app. Hotkey, audio capture, overlay, menu bar, paste, clipboard |
| `KoeApp/VirtualMicPlugin/` | CoreAudio HAL plug-in (`KoeVirtualMic.driver`) for translation pipeline virtual mic |
| `Packages/KoeMLX` | Swift package. Bridges MLX inference to Rust C FFI (Apple Silicon only) |
| `Packages/KoeAppleSpeech` | Swift package. Bridges Apple SpeechAnalyzer to Rust C FFI (macOS 26+) |

## Architecture & Conventions

- **Objective-C ↔ Rust boundary**: `koe-core/src/ffi.rs` defines the C ABI. `koe-core/build.rs` runs **cbindgen** to auto-generate `koe-core/target/koe_core.h`. **Never edit the header manually.** Objective-C wrapper is `KoeApp/Koe/Bridge/SPRustBridge.{h,m}`.
- **Session state machine**: Strict state machine in `koe-core/src/session.rs`. Invalid transitions return `KoeError::SessionInvalidState`.
- **ASR factory**: `koe-core/src/asr_factory.rs` is the single place that maps `cfg.asr.provider` to a provider instance. Add new providers there + in `koe-asr`.
- **Feature flags** (in `koe-core/Cargo.toml`): `mlx`, `apple-speech`, `sherpa-onnx`. Default enables all three.
- **Xcode schemes & features**:
  - `Koe` → default features
  - `Koe-lite` → `--no-default-features --features "apple-speech"`
  - `Koe-x86` → `--no-default-features --features "sherpa-onnx,apple-speech"` (target `x86_64-apple-darwin`)
- **Virtual mic shared buffer**: `koe-core/src/translation/output_bridge.rs` and `KoeApp/VirtualMicPlugin/Sources/shared_buffer_protocol.h` define the same 48-byte header (magic `KOEV`, seqlock at offset 20). File: `/tmp/koe/virtual_mic_output.bin`. Reader tolerates writer churn via seqlock (odd sequence = retry).
- **Buffer sizing**: `output_buffer_frames` defaults to **30 s @ 48 kHz**. TTS dumps entire utterances at once; the HAL reader consumes at real-time rate. Do **not** lower this without changing the writer to chunk, or the writer will lap the reader and only the tail will play.

## Testing

```bash
cargo test --workspace          # All Rust tests
cargo test -p koe-asr --test <file>  # Single test file
```

## Configuration & Runtime

All user config lives in `~/.koe/` (auto-generated on first launch):
- `config.yaml` — ASR provider, LLM profiles, hotkey, feedback
- `dictionary.txt` — one term per line (ASR hotwords + LLM correction context)
- `system_prompt.txt` / `user_prompt.txt` — LLM prompt customization
- `history.db` — SQLite usage statistics
- `models/` — local model manifests and downloaded weights

Config changes take effect automatically on the next hotkey press. No restart required.

## Commit & Release

- Follow [Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <short summary>`
- Common scopes: `asr`, `llm`, `ui`, `config`, `setup`, `overlay`
- For release-worthy user-facing changes, update both `CHANGELOG.md` and `docs/update-feed.json`
- PRs must still build with `make build` and should verify both hold-to-talk and tap-to-toggle modes
