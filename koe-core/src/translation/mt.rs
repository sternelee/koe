use crate::errors::{KoeError, Result};
use crate::translation::config::{MtConfig, MtProvider};
use reqwest::Client;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
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

/// Machine translation client using an OpenAI-compatible chat completions API.
pub struct MtClient {
    client: Client,
    config: MtConfig,
}

impl MtClient {
    pub fn new(client: Client, config: MtConfig) -> Self {
        Self { client, config }
    }

    /// Translate `text` into the target language.
    pub async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }

        match self.config.provider {
            MtProvider::OpenAiCompatible => self.translate_openai_compatible(text, target_lang).await,
            MtProvider::Apple => self.translate_apple(text, source_lang, target_lang),
        }
    }

    async fn translate_openai_compatible(&self, text: &str, target_lang: &str) -> Result<String> {
        let system_prompt = self.config.system_prompt.replace("{target_language}", target_lang);
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));

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
        let source_lang = CString::new(source_lang)
            .map_err(|_| KoeError::LlmFailed("source language contains NUL byte".to_string()))?;
        let target_lang = CString::new(target_lang)
            .map_err(|_| KoeError::LlmFailed("target language contains NUL byte".to_string()))?;

        let ptr = unsafe {
            koe_apple_translation_translate(
                source_text.as_ptr(),
                source_lang.as_ptr(),
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
}
