# koe-asr

Streaming ASR (Automatic Speech Recognition) library for Rust with multiple provider backends.

## Providers

| Provider | Backend | Feature Flag | Connection |
|---|---|---|---|
| `DoubaoWsProvider` | Volcengine / Doubao | *(default)* | WebSocket |
| `DoubaoImeProvider` | Doubao IME (free, device-registered) | *(default)* | WebSocket |
| `QwenAsrProvider` | Alibaba Qwen / Paraformer | *(default)* | WebSocket |
| `AppleSpeechProvider` | macOS Speech Framework | `apple-speech` | Local |
| `MlxProvider` | MLX Whisper (Apple Silicon) | `mlx` | Local |
| `SherpaOnnxProvider` | Sherpa-ONNX | `sherpa-onnx` | Local |

## Usage

```rust
use koe_asr::{AsrConfig, AsrEvent, AsrProvider, DoubaoWsProvider, TranscriptAggregator};

let config = AsrConfig {
    app_key: "your-app-key".into(),
    access_key: "your-access-key".into(),
    ..Default::default()
};

let mut asr = DoubaoWsProvider::new();
asr.connect(&config).await?;

// Push PCM 16-bit mono audio frames
asr.send_audio(&pcm_data).await?;
asr.finish_input().await?;

let mut aggregator = TranscriptAggregator::new();
loop {
    match asr.next_event().await? {
        AsrEvent::Interim(text) => aggregator.update_interim(&text),
        AsrEvent::Definite(text) => aggregator.update_definite(&text),
        AsrEvent::Final(text) => { aggregator.update_final(&text); break; }
        AsrEvent::Closed => break,
        _ => {}
    }
}

println!("{}", aggregator.best_text());
asr.close().await?;
```

## Provider Trait

All providers implement the `AsrProvider` trait:

```rust
#[async_trait]
pub trait AsrProvider: Send {
    async fn connect(&mut self, config: &AsrConfig) -> Result<()>;
    async fn send_audio(&mut self, frame: &[u8]) -> Result<()>;
    async fn finish_input(&mut self) -> Result<()>;
    async fn next_event(&mut self) -> Result<AsrEvent>;
    async fn close(&mut self) -> Result<()>;
}
```

## Events

- `Connected` — connection established
- `Interim(String)` — partial recognition result, may change
- `Definite(String)` — confirmed sentence from two-pass recognition
- `Final(String)` — final result for the session
- `Error(String)` — server-side error
- `Closed` — connection closed

## License

MIT
