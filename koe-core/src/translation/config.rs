use serde::{Deserialize, Serialize};

/// Configuration for the real-time translation engine.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TranslationConfig {
    /// Target language for translation (e.g., "en", "zh", "ja").
    pub target_language: String,
    /// Source language for ASR (e.g., "zh", "en", "auto").
    pub source_language: String,
    /// VAD energy threshold (0.0–1.0, relative to i16 max).
    pub vad_energy_threshold: f32,
    /// Minimum speech duration in milliseconds to trigger ASR.
    pub min_speech_ms: u64,
    /// Silence duration in milliseconds to end a speech segment.
    pub silence_ms: u64,
    /// Maximum speech segment duration in milliseconds.
    pub max_speech_ms: u64,
    /// MT (machine translation) configuration.
    pub mt: MtConfig,
    /// TTS (text-to-speech) configuration.
    pub tts: TtsConfig,
    /// Audio output sample rate (must match HAL plugin, typically 48000).
    pub output_sample_rate: u32,
    /// Audio output channels (must match HAL plugin, typically 1).
    pub output_channels: u16,
    /// Shared output buffer capacity in frames.
    pub output_buffer_frames: usize,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            target_language: "en".to_string(),
            source_language: "auto".to_string(),
            vad_energy_threshold: 0.01,
            min_speech_ms: 500,
            silence_ms: 800,
            max_speech_ms: 30_000,
            mt: MtConfig::default(),
            tts: TtsConfig::default(),
            output_sample_rate: 48_000,
            output_channels: 1,
            // 30 s @ 48 kHz — large enough to hold an entire TTS utterance so
            // the reader (HAL plug-in) consumes it at real-time playback rate
            // without the writer lapping it and dropping the head of the audio.
            output_buffer_frames: 30 * 48_000,
        }
    }
}

/// Machine translation provider.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MtProvider {
    /// OpenAI-compatible chat-completions endpoint.
    OpenAiCompatible,
    /// Apple Translation.framework on-device translator (macOS 15+).
    Apple,
}

impl Default for MtProvider {
    fn default() -> Self {
        Self::OpenAiCompatible
    }
}

/// Machine translation provider configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct MtConfig {
    pub enabled: bool,
    pub provider: MtProvider,
    /// OpenAI-compatible API base URL.
    pub base_url: String,
    /// API key.
    pub api_key: String,
    /// Model name.
    pub model: String,
    /// System prompt for translation.
    pub system_prompt: String,
    /// Request timeout in milliseconds.
    pub timeout_ms: u64,
}

impl Default for MtConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: MtProvider::OpenAiCompatible,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_string(),
            system_prompt: "You are a professional translator. Translate the user's text into the target language. Preserve meaning, tone, and formatting. Output ONLY the translated text, with no extra commentary.".to_string(),
            timeout_ms: 10_000,
        }
    }
}

/// Text-to-speech provider configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsProvider {
    ElevenLabs,
    #[serde(alias = "minimax")]
    MiniMax,
}

impl Default for TtsProvider {
    fn default() -> Self {
        TtsProvider::ElevenLabs
    }
}

/// TTS configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub provider: TtsProvider,
    /// API key.
    pub api_key: String,
    /// Voice ID.
    pub voice_id: String,
    /// Model ID (provider-specific).
    pub model: String,
    /// TTS endpoint base URL (for MiniMax or self-hosted).
    pub base_url: String,
    /// Playback speed multiplier.
    pub speed: f32,
    /// Request timeout in milliseconds.
    pub timeout_ms: u64,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: TtsProvider::ElevenLabs,
            api_key: String::new(),
            voice_id: String::new(),
            model: "eleven_multilingual_v2".to_string(),
            base_url: "https://api.elevenlabs.io".to_string(),
            speed: 1.0,
            timeout_ms: 30_000,
        }
    }
}
