/// Configuration for an ASR session.
#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub url: String,
    pub app_key: String,
    pub access_key: String,
    pub resource_id: String,
    pub sample_rate_hz: u32,
    pub connect_timeout_ms: u64,
    pub final_wait_timeout_ms: u64,
    pub enable_ddc: bool,
    pub enable_itn: bool,
    pub enable_punc: bool,
    pub enable_nonstream: bool,
    pub hotwords: Vec<String>,
    pub model_dir: Option<String>,
    pub provider: Option<String>,
    pub streaming_mode: Option<String>,
    pub vad_threshold: Option<f32>,
    pub vad_min_speech_duration: Option<f32>,
    pub vad_min_silence_duration: Option<f32>,
    pub vad_max_speech_duration: Option<f32>,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            url: "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async".into(),
            app_key: String::new(),
            access_key: String::new(),
            resource_id: "volc.seedasr.sauc.duration".into(),
            sample_rate_hz: 16000,
            connect_timeout_ms: 3000,
            final_wait_timeout_ms: 5000,
            enable_ddc: true,
            enable_itn: true,
            enable_punc: true,
            enable_nonstream: true,
            hotwords: Vec::new(),
            model_dir: None,
            provider: None,
            streaming_mode: None,
            vad_threshold: None,
            vad_min_speech_duration: None,
            vad_min_silence_duration: None,
            vad_max_speech_duration: None,
        }
    }
}
