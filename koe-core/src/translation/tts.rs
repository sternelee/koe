use crate::errors::{KoeError, Result};
use crate::translation::config::{TtsConfig, TtsProvider};
use reqwest::Client;
use serde_json::{json, Value};
#[cfg(feature = "sherpa-onnx")]
use std::collections::HashMap;
use std::time::Duration;

#[cfg(feature = "sherpa-onnx")]
const KOKORO_PRESET_VOICES: &[(&str, i32)] = &[
    ("af_alloy", 0),
    ("af_aoede", 1),
    ("af_bella", 2),
    ("af_heart", 3),
    ("af_jessica", 4),
    ("af_kore", 5),
    ("af_nicole", 6),
    ("af_nova", 7),
    ("af_river", 8),
    ("af_sarah", 9),
    ("af_sky", 10),
    ("am_adam", 11),
    ("am_echo", 12),
    ("am_eric", 13),
    ("am_fenrir", 14),
    ("am_liam", 15),
    ("am_michael", 16),
    ("am_onyx", 17),
    ("am_puck", 18),
    ("am_santa", 19),
    ("bf_alice", 20),
    ("bf_emma", 21),
    ("bf_isabella", 22),
    ("bf_lily", 23),
    ("bm_daniel", 24),
    ("bm_fable", 25),
    ("bm_george", 26),
    ("bm_lewis", 27),
    ("ef_dora", 28),
    ("em_alex", 29),
    ("ff_siwis", 30),
    ("hf_alpha", 31),
    ("hf_beta", 32),
    ("hm_omega", 33),
    ("hm_psi", 34),
    ("if_sara", 35),
    ("im_nicola", 36),
    ("jf_alpha", 37),
    ("jf_gongitsune", 38),
    ("jf_nezumi", 39),
    ("jf_tebukuro", 40),
    ("jm_kumo", 41),
    ("pf_dora", 42),
    ("pm_alex", 43),
    ("pm_santa", 44),
    ("zf_xiaobei", 45),
    ("zf_xiaoni", 46),
    ("zf_xiaoxiao", 47),
    ("zf_xiaoyi", 48),
    ("zm_yunjian", 49),
    ("zm_yunxi", 50),
    ("zm_yunxia", 51),
    ("zm_yunyang", 52),
];

#[cfg(feature = "sherpa-onnx")]
enum LocalTtsBackend {
    Kokoro(KokoroOnnxBackend),
    Supertonic(SupertonicOnnxBackend),
}

/// Text-to-speech client that supports multiple cloud and local providers.
pub struct TtsClient {
    client: Client,
    config: TtsConfig,
    #[cfg(feature = "sherpa-onnx")]
    local_backend: Option<LocalTtsBackend>,
}

impl TtsClient {
    pub fn new(client: Client, config: TtsConfig) -> Self {
        #[cfg(feature = "sherpa-onnx")]
        let local_backend = Self::build_local_backend(&config);

        Self {
            client,
            config,
            #[cfg(feature = "sherpa-onnx")]
            local_backend,
        }
    }

    #[cfg(feature = "sherpa-onnx")]
    fn build_local_backend(config: &TtsConfig) -> Option<LocalTtsBackend> {
        match config.provider {
            TtsProvider::KokoroOnnx => Self::load_kokoro_backend(config),
            TtsProvider::SupertonicOnnx => Self::load_supertonic_backend(config),
            _ => None,
        }
    }

    #[cfg(feature = "sherpa-onnx")]
    fn load_kokoro_backend(config: &TtsConfig) -> Option<LocalTtsBackend> {
        if config.model.is_empty() {
            log::warn!("[tts] Kokoro ONNX provider selected but model path is empty");
            return None;
        }

        let model_dir = crate::config::resolve_model_dir(&config.model);
        let speaker_id = Self::kokoro_speaker_id(config);
        match KokoroOnnxBackend::new(&model_dir, speaker_id, config.speed) {
            Ok(backend) => {
                log::info!(
                    "[tts] Kokoro ONNX backend loaded from {} (speaker_id={speaker_id})",
                    model_dir.display()
                );
                Some(LocalTtsBackend::Kokoro(backend))
            }
            Err(e) => {
                log::warn!("[tts] Failed to load Kokoro ONNX backend: {e}");
                None
            }
        }
    }

    #[cfg(feature = "sherpa-onnx")]
    fn load_supertonic_backend(config: &TtsConfig) -> Option<LocalTtsBackend> {
        if config.model.is_empty() {
            log::warn!("[tts] Supertonic ONNX provider selected but model path is empty");
            return None;
        }

        let model_dir = crate::config::resolve_model_dir(&config.model);
        match SupertonicOnnxBackend::new(&model_dir, config.speaker_id, config.speed) {
            Ok(backend) => {
                log::info!(
                    "[tts] Supertonic ONNX backend loaded from {} (speaker_id={})",
                    model_dir.display(),
                    config.speaker_id
                );
                Some(LocalTtsBackend::Supertonic(backend))
            }
            Err(e) => {
                log::warn!("[tts] Failed to load Supertonic ONNX backend: {e}");
                None
            }
        }
    }

    /// Synthesize `text` into f32 PCM audio.
    ///
    /// Returns `(samples, sample_rate)`.
    pub async fn synthesize(&self, text: &str, language: Option<&str>) -> Result<(Vec<f32>, u32)> {
        if text.trim().is_empty() {
            return Ok((Vec::new(), self.config.sample_rate()));
        }

        match self.config.provider {
            TtsProvider::ElevenLabs => self.elevenlabs_synthesize(text).await,
            TtsProvider::MiniMax => self.minimax_synthesize(text).await,
            TtsProvider::KokoroOnnx => self.kokoro_synthesize(text).await,
            TtsProvider::SupertonicOnnx => self.supertonic_synthesize(text, language).await,
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

    #[cfg(feature = "sherpa-onnx")]
    fn kokoro_speaker_id(config: &TtsConfig) -> i32 {
        let preset = config.preset_voice.trim();
        if preset.is_empty() {
            return config.speaker_id;
        }

        KOKORO_PRESET_VOICES
            .iter()
            .find(|(voice_id, _)| *voice_id == preset)
            .map(|(_, sid)| *sid)
            .unwrap_or(config.speaker_id)
    }

    async fn kokoro_synthesize(&self, _text: &str) -> Result<(Vec<f32>, u32)> {
        #[cfg(feature = "sherpa-onnx")]
        {
            if let Some(LocalTtsBackend::Kokoro(ref kokoro)) = self.local_backend {
                let text = _text.to_string();
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

    async fn supertonic_synthesize(
        &self,
        _text: &str,
        language: Option<&str>,
    ) -> Result<(Vec<f32>, u32)> {
        #[cfg(feature = "sherpa-onnx")]
        {
            let lang = language.and_then(supertonic_language_code).ok_or_else(|| {
                KoeError::Config(format!(
                    "Supertonic ONNX supports only en, ko, es, pt, fr; got {:?}",
                    language.unwrap_or("")
                ))
            })?;

            if let Some(LocalTtsBackend::Supertonic(ref supertonic)) = self.local_backend {
                let text = _text.to_string();
                let supertonic = supertonic.clone();
                let lang = lang.to_string();
                tokio::task::spawn_blocking(move || supertonic.synthesize(&text, &lang))
                    .await
                    .map_err(|e| KoeError::LlmFailed(format!("TTS task failed: {e}")))?
            } else {
                Err(KoeError::Config(
                    "Supertonic ONNX backend not initialized. Check model path.".to_string(),
                ))
            }
        }
        #[cfg(not(feature = "sherpa-onnx"))]
        {
            let _ = language;
            Err(KoeError::Config(
                "Supertonic ONNX TTS requires the sherpa-onnx feature".to_string(),
            ))
        }
    }
}

pub(crate) fn supertonic_language_code(language: &str) -> Option<&'static str> {
    let normalized = language.trim().to_ascii_lowercase();
    let base = normalized.split(['-', '_']).next().unwrap_or("");
    match base {
        "en" => Some("en"),
        "ko" => Some("ko"),
        "es" => Some("es"),
        "pt" => Some("pt"),
        "fr" => Some("fr"),
        _ => None,
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
            TtsProvider::KokoroOnnx => 24_000,
            #[cfg(feature = "sherpa-onnx")]
            TtsProvider::SupertonicOnnx => 44_100,
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
unsafe impl Send for KokoroOnnxBackendInner {}
#[cfg(feature = "sherpa-onnx")]
unsafe impl Sync for KokoroOnnxBackendInner {}

#[cfg(feature = "sherpa-onnx")]
impl KokoroOnnxBackend {
    pub fn new(model_dir: &std::path::Path, speaker_id: i32, speed: f32) -> Result<Self> {
        let config = build_kokoro_config(model_dir)?;
        let tts = sherpa_onnx::OfflineTts::create(&config)
            .ok_or_else(|| KoeError::Config("OfflineTts::create failed for Kokoro".to_string()))?;

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

// =============================================================================
// Supertonic ONNX backend (sherpa-onnx)
// =============================================================================

#[cfg(feature = "sherpa-onnx")]
#[derive(Clone)]
pub struct SupertonicOnnxBackend {
    inner: std::sync::Arc<SupertonicOnnxBackendInner>,
}

#[cfg(feature = "sherpa-onnx")]
struct SupertonicOnnxBackendInner {
    tts: sherpa_onnx::OfflineTts,
    speaker_id: i32,
    speed: f32,
    num_steps: i32,
}

#[cfg(feature = "sherpa-onnx")]
unsafe impl Send for SupertonicOnnxBackendInner {}
#[cfg(feature = "sherpa-onnx")]
unsafe impl Sync for SupertonicOnnxBackendInner {}

#[cfg(feature = "sherpa-onnx")]
impl SupertonicOnnxBackend {
    pub fn new(model_dir: &std::path::Path, speaker_id: i32, speed: f32) -> Result<Self> {
        let config = build_supertonic_config(model_dir)?;
        let tts = sherpa_onnx::OfflineTts::create(&config).ok_or_else(|| {
            KoeError::Config("OfflineTts::create failed for Supertonic".to_string())
        })?;

        log::info!(
            "[tts] Supertonic loaded: sample_rate={} num_speakers={}",
            tts.sample_rate(),
            tts.num_speakers(),
        );

        Ok(Self {
            inner: std::sync::Arc::new(SupertonicOnnxBackendInner {
                tts,
                speaker_id,
                speed,
                num_steps: 8,
            }),
        })
    }

    pub fn synthesize(&self, text: &str, lang: &str) -> Result<(Vec<f32>, u32)> {
        let mut extra = HashMap::with_capacity(1);
        extra.insert("lang".to_string(), Value::String(lang.to_string()));

        let gen_cfg = sherpa_onnx::GenerationConfig {
            sid: self.inner.speaker_id,
            speed: self.inner.speed,
            num_steps: self.inner.num_steps,
            extra: Some(extra),
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
        KoeError::Config(format!("kokoro model.onnx not found in {}", dir.display()))
    })?;
    let voices = require_file(dir, "voices.bin")?;
    let tokens = require_file(dir, "tokens.txt")?;

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
fn build_supertonic_config(dir: &std::path::Path) -> Result<sherpa_onnx::OfflineTtsConfig> {
    let duration_predictor = best_onnx(dir, "duration_predictor").ok_or_else(|| {
        KoeError::Config(format!(
            "supertonic duration_predictor.onnx not found in {}",
            dir.display()
        ))
    })?;
    let text_encoder = best_onnx(dir, "text_encoder").ok_or_else(|| {
        KoeError::Config(format!(
            "supertonic text_encoder.onnx not found in {}",
            dir.display()
        ))
    })?;
    let vector_estimator = best_onnx(dir, "vector_estimator").ok_or_else(|| {
        KoeError::Config(format!(
            "supertonic vector_estimator.onnx not found in {}",
            dir.display()
        ))
    })?;
    let vocoder = best_onnx(dir, "vocoder").ok_or_else(|| {
        KoeError::Config(format!(
            "supertonic vocoder.onnx not found in {}",
            dir.display()
        ))
    })?;

    Ok(sherpa_onnx::OfflineTtsConfig {
        model: sherpa_onnx::OfflineTtsModelConfig {
            supertonic: sherpa_onnx::OfflineTtsSupertonicModelConfig {
                duration_predictor: Some(duration_predictor),
                text_encoder: Some(text_encoder),
                vector_estimator: Some(vector_estimator),
                vocoder: Some(vocoder),
                tts_json: Some(require_file(dir, "tts.json")?),
                unicode_indexer: Some(require_file(dir, "unicode_indexer.bin")?),
                voice_style: Some(require_file(dir, "voice.bin")?),
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

#[cfg(all(test, feature = "sherpa-onnx"))]
mod tests {
    use super::*;

    #[test]
    fn kokoro_preset_voice_maps_to_expected_speaker_id() {
        let mut config = TtsConfig::default();
        config.provider = TtsProvider::KokoroOnnx;
        config.preset_voice = "af_heart".into();
        config.speaker_id = 99;
        assert_eq!(TtsClient::kokoro_speaker_id(&config), 3);
    }

    #[test]
    fn kokoro_unknown_preset_falls_back_to_numeric_speaker_id() {
        let mut config = TtsConfig::default();
        config.provider = TtsProvider::KokoroOnnx;
        config.preset_voice = "unknown_voice".into();
        config.speaker_id = 12;
        assert_eq!(TtsClient::kokoro_speaker_id(&config), 12);
    }

    #[test]
    fn supertonic_language_code_normalizes_supported_locales() {
        assert_eq!(supertonic_language_code("en"), Some("en"));
        assert_eq!(supertonic_language_code("PT-BR"), Some("pt"));
        assert_eq!(supertonic_language_code("fr_CA"), Some("fr"));
    }

    #[test]
    fn supertonic_language_code_rejects_unsupported_locales() {
        assert_eq!(supertonic_language_code("zh-CN"), None);
        assert_eq!(supertonic_language_code("ja"), None);
        assert_eq!(supertonic_language_code(""), None);
    }
}
