use crate::errors::{KoeError, Result};
use crate::translation::config::{TtsConfig, TtsProvider};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

/// Text-to-speech client that supports multiple cloud providers.
pub struct TtsClient {
    client: Client,
    config: TtsConfig,
}

impl TtsClient {
    pub fn new(client: Client, config: TtsConfig) -> Self {
        Self { client, config }
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
        24_000
    }
}
