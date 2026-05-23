use crate::errors::{KoeError, Result};
use crate::translation::config::{MtConfig, MtProvider};
use crate::translation::local_mt::{self, LocalMtBackend};
use reqwest::Client;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;
use std::time::Duration;

extern "C" {
    fn koe_apple_translation_is_available() -> i32;
    fn koe_apple_translation_translate(
        source_text: *const c_char,
        source_lang: *const c_char,
        target_lang: *const c_char,
    ) -> *mut c_char;
    fn koe_apple_translation_free_string(ptr: *mut c_char);
}

/// Machine translation client.
pub struct MtClient {
    client: Client,
    config: MtConfig,
    local_backend: Option<Arc<dyn LocalMtBackend>>,
    local_backend_error: Option<String>,
}

impl MtClient {
    pub fn new(client: Client, config: MtConfig, source_lang: Option<&str>) -> Self {
        let (local_backend, local_backend_error) = if config.provider == MtProvider::Local {
            if config.model.trim().is_empty() {
                (None, Some("local MT model path is empty".to_string()))
            } else {
                let model_path = crate::config::resolve_model_dir(&config.model);
                match local_mt::load_backend(&model_path, source_lang) {
                    Ok(backend) => (Some(backend), None),
                    Err(err) => (None, Some(err.to_string())),
                }
            }
        } else {
            (None, None)
        };

        Self {
            client,
            config,
            local_backend,
            local_backend_error,
        }
    }

    /// Translate `text` into the target language.
    pub async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }

        let normalized_target = target_lang.trim();
        if normalized_target.is_empty() {
            return Err(KoeError::LlmFailed("target language is empty".to_string()));
        }

        match self.config.provider {
            MtProvider::OpenAiCompatible => {
                self.translate_openai_compatible(text, normalized_target).await
            }
            MtProvider::Apple => self.translate_apple(text, source_lang, normalized_target),
            MtProvider::Local => self.translate_local(text, normalized_target),
        }
    }

    async fn translate_openai_compatible(&self, text: &str, target_lang: &str) -> Result<String> {
        let system_prompt = self
            .config
            .system_prompt
            .replace("{target_language}", target_lang);
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );

        let body = json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": text }
            ],
            "temperature": 0.3,
            "max_tokens": 1024,
        });

        let mut builder = self
            .client
            .post(&url)
            .timeout(Duration::from_millis(self.config.timeout_ms))
            .header("Content-Type", "application/json");

        if !self.config.api_key.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", self.config.api_key));
        }

        let response = builder
            .json(&body)
            .send()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("MT request failed: {e}")))?;

        let status = response.status();
        let json: Value = response
            .json()
            .await
            .map_err(|e| KoeError::LlmFailed(format!("MT response parse failed: {e}")))?;

        if !status.is_success() {
            let msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown MT error");
            return Err(KoeError::LlmFailed(format!("MT HTTP {status}: {msg}")));
        }

        let translated = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        Ok(translated)
    }

    fn translate_apple(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        if unsafe { koe_apple_translation_is_available() } == 0 {
            return Err(KoeError::LlmFailed(
                "Apple Translation is unavailable on this macOS version".to_string(),
            ));
        }

        let source_text = CString::new(text)
            .map_err(|_| KoeError::LlmFailed("MT input contains NUL byte".to_string()))?;
        let source_lang = if source_lang.trim().is_empty() || source_lang.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(
                CString::new(source_lang)
                    .map_err(|_| KoeError::LlmFailed("source language contains NUL byte".to_string()))?,
            )
        };
        let target_lang = CString::new(target_lang)
            .map_err(|_| KoeError::LlmFailed("target language contains NUL byte".to_string()))?;

        let ptr = unsafe {
            koe_apple_translation_translate(
                source_text.as_ptr(),
                source_lang.as_ref().map_or(std::ptr::null(), |lang| lang.as_ptr()),
                target_lang.as_ptr(),
            )
        };

        if ptr.is_null() {
            return Err(KoeError::LlmFailed(
                "Apple Translation failed (empty response)".to_string(),
            ));
        }

        let translated = unsafe {
            let value = CStr::from_ptr(ptr).to_string_lossy().to_string();
            koe_apple_translation_free_string(ptr);
            value
        };

        if translated.starts_with("[error]") {
            return Err(KoeError::LlmFailed(
                translated
                    .strip_prefix("[error]")
                    .unwrap_or(&translated)
                    .trim()
                    .to_string(),
            ));
        }

        Ok(translated.trim().to_string())
    }

    fn translate_local(&self, text: &str, target_lang: &str) -> Result<String> {
        let backend = self.local_backend.as_ref().ok_or_else(|| {
            KoeError::LlmFailed(
                self.local_backend_error
                    .clone()
                    .unwrap_or_else(|| "local MT backend is not initialized".to_string()),
            )
        })?;
        backend.translate(text, target_lang)
    }
}
