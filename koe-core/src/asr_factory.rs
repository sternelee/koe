use crate::config::{self, Config};
#[cfg(feature = "apple-speech")]
use koe_asr::{AppleSpeechConfig, AppleSpeechProvider};
use koe_asr::{
    AsrConfig, AsrProvider, DoubaoImeProvider, DoubaoWsProvider, QwenAsrProvider,
};
#[cfg(feature = "mlx")]
use koe_asr::{MlxConfig, MlxProvider};
#[cfg(feature = "sherpa-onnx")]
use koe_asr::{SherpaOnnxConfig, SherpaOnnxProvider};

/// Build an ASR provider and its config from the global configuration.
///
/// Kept in its own module so that adding ASR providers (the part of the codebase
/// upstream changes most frequently) does not collide with translation-related
/// edits in `lib.rs`.
pub fn build_asr_provider(cfg: &Config, dictionary: &[String]) -> (AsrConfig, Box<dyn AsrProvider>) {
    let asr_provider_name = cfg.asr.provider.clone();
    match asr_provider_name.as_str() {
        "doubaoime" => {
            let ime = &cfg.asr.doubaoime;
            let credential_path = if std::path::Path::new(&ime.credential_path).is_absolute() {
                ime.credential_path.clone()
            } else {
                config::config_dir()
                    .join(&ime.credential_path)
                    .to_string_lossy()
                    .to_string()
            };
            let mut custom_headers = std::collections::HashMap::new();
            custom_headers.insert("credential_path".to_string(), credential_path);
            let config = AsrConfig {
                url: String::new(),
                app_key: String::new(),
                access_key: String::new(),
                api_key: String::new(),
                resource_id: String::new(),
                sample_rate_hz: 16000,
                connect_timeout_ms: ime.connect_timeout_ms,
                final_wait_timeout_ms: ime.final_wait_timeout_ms,
                enable_ddc: false,
                enable_itn: false,
                enable_punc: true,
                enable_nonstream: false,
                hotwords: Vec::new(),
                language: ime.language.clone(),
                custom_headers,
                end_window_size: None,
                force_to_speech_time: None,
                vad_segment_duration: None,
                output_zh_variant: None,
                enable_accelerate_text: false,
                accelerate_score: None,
                context_messages: Vec::new(),
            };
            (config, Box::new(DoubaoImeProvider::new()))
        }
        "qwen" => {
            let qwen = &cfg.asr.qwen;
            let config = AsrConfig {
                url: qwen.url.clone(),
                app_key: qwen.model.clone(),
                access_key: qwen.api_key.clone(),
                api_key: String::new(),
                resource_id: String::new(),
                sample_rate_hz: 16000,
                connect_timeout_ms: qwen.connect_timeout_ms,
                final_wait_timeout_ms: qwen.final_wait_timeout_ms,
                enable_ddc: false,
                enable_itn: false,
                enable_punc: false,
                enable_nonstream: false,
                hotwords: Vec::new(),
                language: Some(qwen.language.clone()),
                custom_headers: qwen.headers.clone(),
                end_window_size: None,
                force_to_speech_time: None,
                vad_segment_duration: None,
                output_zh_variant: None,
                enable_accelerate_text: false,
                accelerate_score: None,
                context_messages: Vec::new(),
            };
            (config, Box::new(QwenAsrProvider::new()))
        }
        #[cfg(feature = "mlx")]
        "mlx" => {
            let mlx = &cfg.asr.mlx;
            let model_path = config::resolve_model_dir(&mlx.model)
                .to_string_lossy()
                .to_string();
            let mlx_config = MlxConfig {
                model_path,
                language: mlx.language.clone(),
                delay_preset: mlx.delay_preset.clone(),
            };
            (AsrConfig::default(), Box::new(MlxProvider::new(mlx_config)))
        }
        #[cfg(feature = "sherpa-onnx")]
        "sherpa-onnx" => {
            let s = &cfg.asr.sherpa_onnx;
            let model_dir = config::resolve_model_dir(&s.model);
            let sherpa_config = SherpaOnnxConfig {
                model_dir,
                num_threads: s.num_threads,
                hotwords: dictionary.to_vec(),
                hotwords_score: s.hotwords_score,
                endpoint_silence: s.endpoint_silence,
            };
            (
                AsrConfig::default(),
                Box::new(SherpaOnnxProvider::new(sherpa_config)),
            )
        }
        #[cfg(feature = "apple-speech")]
        "apple-speech" => {
            let as_cfg = &cfg.asr.apple_speech;
            let apple_config = AppleSpeechConfig {
                locale: as_cfg.locale.clone(),
                contextual_strings: dictionary.to_vec(),
            };
            (
                AsrConfig::default(),
                Box::new(AppleSpeechProvider::new(apple_config)),
            )
        }
        _ => {
            let doubao = &cfg.asr.doubao;
            let config = AsrConfig {
                url: doubao.url.clone(),
                app_key: doubao.app_key.clone(),
                access_key: doubao.access_key.clone(),
                api_key: doubao.api_key.clone(),
                resource_id: doubao.resource_id.clone(),
                sample_rate_hz: 16000,
                connect_timeout_ms: doubao.connect_timeout_ms,
                final_wait_timeout_ms: doubao.final_wait_timeout_ms,
                enable_ddc: doubao.enable_ddc,
                enable_itn: doubao.enable_itn,
                enable_punc: doubao.enable_punc,
                enable_nonstream: doubao.enable_nonstream,
                hotwords: dictionary.to_vec(),
                language: doubao.language.clone(),
                custom_headers: doubao.headers.clone(),
                end_window_size: doubao.end_window_size,
                force_to_speech_time: doubao.force_to_speech_time,
                vad_segment_duration: doubao.vad_segment_duration,
                output_zh_variant: doubao.output_zh_variant.clone(),
                enable_accelerate_text: doubao.enable_accelerate_text,
                accelerate_score: doubao.accelerate_score,
                context_messages: Vec::new(),
            };
            (config, Box::new(DoubaoWsProvider::new()))
        }
    }
}
