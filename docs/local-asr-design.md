# Local ASR Support Design

## Overview

Add support for local ASR models (SenseVoice and Whisper) using sherpa-onnx, complementing the existing Doubao cloud ASR.

## Architecture

```
koe-asr/src/
├── lib.rs
├── provider.rs     # AsrProvider trait (unchanged)
├── doubao.rs      # Doubao cloud ASR (unchanged)
├── sherpa_onnx.rs # Local ASR via sherpa-onnx
├── config.rs      # Extended with provider selection
├── error.rs
├── event.rs
└── transcript.rs
```

## Provider Selection

Config via `config.yaml`:

```yaml
asr:
  provider: "doubao" | "sensevoice" | "whisper"
  
  doubao:
    # existing cloud config
    
  local:
    model_dir: "~/.koe/models"
    streaming_mode: "vad" | "interval"
    vad_threshold: 0.5
    vad_min_speech_duration: 0.25
    vad_min_silence_duration: 0.5
    vad_max_speech_duration: 30.0
```

## Model Storage

```
~/.koe/models/
├── sensevoice/
│   ├── model.int8.onnx
│   └── tokens.txt
├── whisper/
│   ├── tiny.en-encoder.int8.onnx
│   ├── tiny.en-decoder.int8.onnx
│   └── tiny.en-tokens.txt
└── silero_vad.onnx
```

## SherpaOnnxProvider Implementation

Implements `AsrProvider` trait:
- `connect()`: Initialize recognizer with model config
- `send_audio()`: Accumulate audio frames
- `finish_input()`: Trigger final recognition
- `next_event()`: Return interim/final results
- `close()`: Release resources

Supports:
- SenseVoice (multi-language, default)
- Whisper tiny.en (English only)
- VAD-based streaming with Silero VAD
- Interval-based streaming

## Key Design Decisions

1. Auto-download models on first use (like coli)
2. VAD mode default, interval mode available
3. Provider selected via config.yaml
4. sherpa-onnx runs in a separate thread with tokio runtime for async compatibility
