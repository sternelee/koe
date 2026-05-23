use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::config::AsrConfig;
use crate::error::{AsrError, Result};
use crate::event::AsrEvent;
use crate::provider::AsrProvider;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

static WHISPER_CONTEXT_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<WhisperContext>>>> =
    OnceLock::new();

#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub model_dir: PathBuf,
    pub language: Option<String>,
}

pub struct WhisperProvider {
    config: WhisperConfig,
    context: Option<Arc<WhisperContext>>,
    buffered_audio: Vec<f32>,
    pending_events: VecDeque<AsrEvent>,
}

impl WhisperProvider {
    pub fn new(config: WhisperConfig) -> Self {
        Self {
            config,
            context: None,
            buffered_audio: Vec::new(),
            pending_events: VecDeque::new(),
        }
    }

    fn cached_context(model_path: &Path) -> Result<Arc<WhisperContext>> {
        let cache = WHISPER_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().unwrap();
        if let Some(ctx) = guard.get(model_path) {
            return Ok(ctx.clone());
        }

        let params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(model_path, params)
            .map_err(|e| AsrError::Connection(format!("failed to load Whisper model: {e}")))?;
        let ctx = Arc::new(ctx);
        guard.insert(model_path.to_path_buf(), ctx.clone());
        Ok(ctx)
    }

    fn model_file(model_dir: &Path) -> Result<PathBuf> {
        if model_dir.is_file() {
            return Ok(model_dir.to_path_buf());
        }

        let entries = std::fs::read_dir(model_dir).map_err(|e| {
            AsrError::Connection(format!(
                "failed to read Whisper model directory {}: {e}",
                model_dir.display()
            ))
        })?;

        let mut candidates: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("ggml-") && name.ends_with(".bin"))
                    .unwrap_or(false)
            })
            .collect();
        candidates.sort();

        candidates.into_iter().next().ok_or_else(|| {
            AsrError::Connection(format!(
                "no ggml Whisper model found in {}",
                model_dir.display()
            ))
        })
    }

    fn transcribe_buffer(&self, ctx: &WhisperContext) -> Result<String> {
        let mut state = ctx
            .create_state()
            .map_err(|e| AsrError::Connection(format!("failed to create Whisper state: {e}")))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(4) as i32,
        );
        params.set_translate(false);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if ctx.is_multilingual() {
            params.set_language(self.config.language.as_deref());
        }
        state
            .full(params, &self.buffered_audio)
            .map_err(|e| AsrError::Connection(format!("Whisper transcription failed: {e}")))?;

        let mut text = String::new();
        for segment in state.as_iter() {
            let segment_text = segment.to_str_lossy().map_err(|e| {
                AsrError::Connection(format!("failed to read Whisper segment: {e}"))
            })?;
            let trimmed = segment_text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(trimmed);
        }
        Ok(text)
    }
}

#[async_trait::async_trait]
impl AsrProvider for WhisperProvider {
    async fn connect(&mut self, _config: &AsrConfig) -> Result<()> {
        let model_path = Self::model_file(&self.config.model_dir)?;
        self.context = Some(Self::cached_context(&model_path)?);
        self.buffered_audio.clear();
        self.pending_events.clear();
        self.pending_events.push_back(AsrEvent::Connected);
        Ok(())
    }

    async fn send_audio(&mut self, frame: &[u8]) -> Result<()> {
        if self.context.is_none() {
            return Err(AsrError::Connection(
                "Whisper provider is not connected".into(),
            ));
        }

        self.buffered_audio.extend(
            frame
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0),
        );
        Ok(())
    }

    async fn finish_input(&mut self) -> Result<()> {
        let ctx = self
            .context
            .as_deref()
            .ok_or_else(|| AsrError::Connection("Whisper provider is not connected".into()))?;

        let text = self.transcribe_buffer(ctx)?;
        if !text.is_empty() {
            self.pending_events.push_back(AsrEvent::Final(text));
        }
        self.pending_events.push_back(AsrEvent::Closed(None));
        self.buffered_audio.clear();
        Ok(())
    }

    async fn next_event(&mut self) -> Result<AsrEvent> {
        self.pending_events
            .pop_front()
            .ok_or_else(|| AsrError::Connection("Whisper provider has no pending events".into()))
    }

    async fn close(&mut self) -> Result<()> {
        self.buffered_audio.clear();
        self.pending_events.clear();
        self.context = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_whisper_model_file_in_directory() {
        let temp_dir = std::env::temp_dir().join(format!(
            "koe-whisper-model-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let model = temp_dir.join("ggml-base.bin");
        std::fs::write(&model, b"stub").unwrap();

        let resolved = WhisperProvider::model_file(&temp_dir).unwrap();
        assert_eq!(resolved, model);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
