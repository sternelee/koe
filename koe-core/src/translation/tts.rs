use crate::errors::{KoeError, Result};
use crate::translation::config::{TtsConfig, TtsProvider};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

/// Text-to-speech client that supports multiple cloud and local providers.
pub struct TtsClient {
    client: Client,
    config: TtsConfig,
    #[cfg(feature = "sherpa-onnx")]
    kokoro: Option<KokoroOnnxBackend>,
}

impl TtsClient {
    pub fn new(client: Client, config: TtsConfig) -> Self {
        #[cfg(feature = "sherpa-onnx")]
        let kokoro = if config.provider == TtsProvider::KokoroOnnx {
            if config.model.is_empty() {
                log::warn!("[tts] Kokoro ONNX provider selected but model path is empty");
                None
            } else {
                let model_dir = crate::config::resolve_model_dir(&config.model);
                match KokoroOnnxBackend::new(&model_dir, config.speaker_id, config.speed) {
                    Ok(backend) => {
                        log::info!("[tts] Kokoro ONNX backend loaded from {}", model_dir.display());
                        Some(backend)
                    }
                    Err(e) => {
                        log::warn!("[tts] Failed to load Kokoro ONNX backend: {e}");
                        None
                    }
                }
            }
        } else {
            None
        };

        Self {
            client,
            config,
            #[cfg(feature = "sherpa-onnx")]
            kokoro,
        }
    }

    /// Synthesize `text` into f32 PCM audio.
    ///
    /// Returns `(samples, sample_rate)`.
    pub async fn synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        if text.trim().is_empty() {
            return Ok((Vec::new(), self.config.sample_rate()));
        }

        match self.config.provider {
            TtsProvider::ElevenLabs => self.elevenlabs_synthesize(text).await,
            TtsProvider::MiniMax => self.minimax_synthesize(text).await,
            TtsProvider::KokoroOnnx => self.kokoro_synthesize(text).await,
        }
    }

    async fn elevenlabs_synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        let url = format!(
            "{}/v1/text-to-speech/{}",
            self.config.base_url.trim_end_matches('/'),
            self.config.voice_id
        );

        let mut body = json!({
            "text": text,
            "model_id": self.config.model,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.5,
            },
            "output_format": "pcm_24000",
        });
        if self.config.speed != 1.0 {
            body["speed"] = json!(self.config.speed);
        }

        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_millis(self.config.timeout_ms))
            .header("Content-Type", "application/json")
            .header("xi-api-key", &self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("TTS request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(KoeError::LlmFailed(format!(
                "TTS HTTP {status}: {body_text}"
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("TTS read body failed: {e}")))?;

        // PCM 24kHz s16le → f32
        let samples = pcm_i16le_to_f32(&bytes);
        let sample_rate = 24_000;

        Ok((samples, sample_rate))
    }

    async fn minimax_synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        let url = format!("{}/v1/t2a_v2", self.config.base_url.trim_end_matches('/'));

        let body = json!({
            "model": self.config.model,
            "text": text,
            "voice_setting": {
                "voice_id": self.config.voice_id,
                "speed": self.config.speed,
            },
            "audio_setting": {
                "sample_rate": 24000,
                "format": "pcm",
            },
        });

        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_millis(self.config.timeout_ms))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("TTS request failed: {e}")))?;

        let status = response.status();
        let json: Value = response
            .json()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("TTS response parse failed: {e}")))?;

        if !status.is_success() {
            let msg = json
                .get("base_resp")
                .and_then(|r| r.get("status_msg"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown TTS error");
            return Err(KoeError::LlmFailed(format!("TTS HTTP {status}: {msg}")));
        }

        let hex_audio = json
            .get("data")
            .and_then(|d| d.get("audio"))
            .and_then(|a| a.as_str())
            .unwrap_or("");

        let bytes = hex::decode(hex_audio)
            .map_err(|e| KoeError::LlmFailed(format!("TTS hex decode failed: {e}")))?;

        let samples = pcm_i16le_to_f32(&bytes);
        let sample_rate = 24_000;

        Ok((samples, sample_rate))
    }

    async fn kokoro_synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        #[cfg(feature = "sherpa-onnx")]
        {
            if let Some(ref kokoro) = self.kokoro {
                let text = text.to_string();
                let kokoro = kokoro.clone();
                tokio::task::spawn_blocking(move || kokoro.synthesize(&text))
                    .await
                    .map_err(|e| KoeError::LlmFailed(format!("TTS task failed: {e}")))?
            } else {
                Err(KoeError::Config(
                    "Kokoro ONNX backend not initialized. Check model path.".to_string(),
                ))
            }
        }
        #[cfg(not(feature = "sherpa-onnx"))]
        {
            Err(KoeError::Config(
                "Kokoro ONNX TTS requires the sherpa-onnx feature".to_string(),
            ))
        }
    }
}

fn pcm_i16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    let mut samples = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let val = i16::from_le_bytes([chunk[0], chunk[1]]);
        samples.push(val as f32 / 32768.0);
    }
    samples
}

impl TtsConfig {
    pub fn sample_rate(&self) -> u32 {
        match self.provider {
            #[cfg(feature = "sherpa-onnx")]
            TtsProvider::KokoroOnnx => 24_000, // Kokoro outputs 24kHz
            _ => 24_000,
        }
    }
}

// =============================================================================
// Kokoro ONNX backend (sherpa-onnx)
// =============================================================================

#[cfg(feature = "sherpa-onnx")]
#[derive(Clone)]
pub struct KokoroOnnxBackend {
    inner: std::sync::Arc<KokoroOnnxBackendInner>,
}

#[cfg(feature = "sherpa-onnx")]
struct KokoroOnnxBackendInner {
    tts: sherpa_onnx::OfflineTts,
    speaker_id: i32,
    speed: f32,
}

#[cfg(feature = "sherpa-onnx")]
// The sherpa-onnx wrapper itself is Send+Sync; we propagate that.
unsafe impl Send for KokoroOnnxBackendInner {}
#[cfg(feature = "sherpa-onnx")]
unsafe impl Sync for KokoroOnnxBackendInner {}

#[cfg(feature = "sherpa-onnx")]
impl KokoroOnnxBackend {
    pub fn new(model_dir: &std::path::Path, speaker_id: i32, speed: f32) -> Result<Self> {
        let config = build_kokoro_config(model_dir)?;
        let tts = sherpa_onnx::OfflineTts::create(&config)
            .ok_or_else(|| KoeError::Config(format!("OfflineTts::create failed for Kokoro")))?;

        log::info!(
            "[tts] Kokoro loaded: sample_rate={} num_speakers={}",
            tts.sample_rate(),
            tts.num_speakers(),
        );

        Ok(Self {
            inner: std::sync::Arc::new(KokoroOnnxBackendInner {
                tts,
                speaker_id,
                speed,
            }),
        })
    }

    pub fn synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
        let gen_cfg = sherpa_onnx::GenerationConfig {
            sid: self.inner.speaker_id,
            speed: self.inner.speed,
            ..Default::default()
        };

        let audio = self
            .inner
            .tts
            .generate_with_config(text, &gen_cfg, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| KoeError::LlmFailed("TTS generate returned None".to_string()))?;

        let samples = audio.samples().to_vec();
        let sample_rate = audio.sample_rate() as u32;
        Ok((samples, sample_rate))
    }
}

#[cfg(feature = "sherpa-onnx")]
fn build_kokoro_config(dir: &std::path::Path) -> Result<sherpa_onnx::OfflineTtsConfig> {
    let model = best_onnx(dir, "model").ok_or_else(|| {
        KoeError::Config(format!(
            "kokoro model.onnx not found in {}",
            dir.display()
        ))
    })?;
    let voices = require_file(dir, "voices.bin")?;
    let tokens = require_file(dir, "tokens.txt")?;

    // Optional data_dir (for Chinese text normalisation)
    let data_dir = {
        let d = dir.join("espeak-ng-data");
        if d.exists() {
            Some(d.to_string_lossy().into_owned())
        } else {
            None
        }
    };

    Ok(sherpa_onnx::OfflineTtsConfig {
        model: sherpa_onnx::OfflineTtsModelConfig {
            kokoro: sherpa_onnx::OfflineTtsKokoroModelConfig {
                model: Some(model),
                voices: Some(voices),
                tokens: Some(tokens),
                data_dir,
                ..Default::default()
            },
            num_threads: 2,
            ..Default::default()
        },
        max_num_sentences: 1,
        ..Default::default()
    })
}

#[cfg(feature = "sherpa-onnx")]
fn best_onnx(dir: &std::path::Path, base: &str) -> Option<String> {
    let int8 = dir.join(format!("{base}.int8.onnx"));
    if int8.exists() {
        return Some(int8.to_string_lossy().into_owned());
    }
    let fp32 = dir.join(format!("{base}.onnx"));
    if fp32.exists() {
        return Some(fp32.to_string_lossy().into_owned());
    }
    None
}

#[cfg(feature = "sherpa-onnx")]
fn require_file(dir: &std::path::Path, name: &str) -> Result<String> {
    let p = dir.join(name);
    if p.exists() {
        Ok(p.to_string_lossy().into_owned())
    } else {
        Err(KoeError::Config(format!(
            "TTS file not found: {}",
            p.display()
        )))
    }
}
