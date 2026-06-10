use crate::errors::{KoeError, Result};
use crate::translation::config::GeminiLiveConfig;
use crate::translation::output_bridge::{AudioFrame, SharedOutputBuffer};
use crate::translation::engine::{now_timestamp_ns, resample_linear};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::connect_async;

const GEMINI_WS_URL: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Gemini Live Translate client.
/// Streams PCM audio to Google Gemini and receives translated audio in real time.
pub struct GeminiLiveClient {
    config: GeminiLiveConfig,
}

impl GeminiLiveClient {
    pub fn new(config: GeminiLiveConfig) -> Self {
        Self { config }
    }

    /// Main loop: connect to Gemini, stream audio from `audio_rx`, and write
    /// translated audio to `output_buffer`. Runs until `stop` is true.
    pub async fn run(
        &self,
        mut audio_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        output_buffer: Arc<SharedOutputBuffer>,
        stop: Arc<AtomicBool>,
        output_sample_rate: u32,
        output_channels: u16,
    ) -> Result<()> {
        let api_key = self.resolve_api_key()?;
        let target_language = self.resolve_target_language();

        let ws_url = format!("{}?key={}", GEMINI_WS_URL, urlencoding::encode(&api_key));
        let request = ws_url
            .into_client_request()
            .map_err(|e| KoeError::LlmFailed(format!("Gemini Live request build failed: {e}")))?;

        log::info!("[gemini-live] connecting…");
        let (ws_stream, _) = timeout(
            Duration::from_millis(self.config.connect_timeout_ms),
            connect_async(request),
        )
        .await
        .map_err(|_| KoeError::LlmFailed("Gemini Live connect timed out".into()))?
        .map_err(|e| KoeError::LlmFailed(format!("Gemini Live connect failed: {e}")))?;
        log::info!("[gemini-live] connected");

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Send setup message.
        let setup = self.build_setup_message(&target_language);
        let setup_json = serde_json::to_string(&setup)
            .map_err(|e| KoeError::LlmFailed(format!("setup serialize: {e}")))?;
        ws_sink
            .send(Message::Text(setup_json))
            .await
            .map_err(|e| KoeError::LlmFailed(format!("setup send failed: {e}")))?;

        // Wait for setup acknowledgement.
        match timeout(Duration::from_millis(self.config.setup_timeout_ms), ws_stream.next()).await
        {
            Ok(Some(Ok(Message::Text(text)))) => {
                log::debug!("[gemini-live] setup response: {text}");
                if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(error) = resp.get("error") {
                        return Err(KoeError::LlmFailed(format!(
                            "Gemini Live setup error: {error}"
                        )));
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                return Err(KoeError::LlmFailed(
                    "Gemini Live closed during setup".into(),
                ));
            }
            Ok(Some(Err(e))) => {
                return Err(KoeError::LlmFailed(format!(
                    "Gemini Live setup read failed: {e}"
                )));
            }
            Ok(None) => {
                return Err(KoeError::LlmFailed(
                    "Gemini Live stream ended during setup".into(),
                ));
            }
            Err(_) => {
                return Err(KoeError::LlmFailed(
                    "Gemini Live setup response timed out".into(),
                ));
            }
            _ => {
                return Err(KoeError::LlmFailed(
                    "Gemini Live unexpected setup response".into(),
                ));
            }
        }
        log::info!("[gemini-live] setup complete");

        let gemini_output_rate = self.config.gemini_output_sample_rate;

        // Spawn writer + reader concurrently.
        let write_stop = stop.clone();
        let writer = async {
            while !write_stop.load(Ordering::SeqCst) {
                match audio_rx.recv().await {
                    Some(bytes) => {
                        let msg = RealtimeInput {
                            realtime_input: RealtimeInputAudio {
                                audio: AudioData {
                                    data: STANDARD.encode(&bytes),
                                    mime_type: "audio/pcm;rate=16000".to_string(),
                                },
                            },
                        };
                        let json = match serde_json::to_string(&msg) {
                            Ok(j) => j,
                            Err(e) => {
                                log::warn!("[gemini-live] audio serialize failed: {e}");
                                continue;
                            }
                        };
                        if let Err(e) = ws_sink.send(Message::Text(json)).await {
                            log::warn!("[gemini-live] audio send failed: {e}");
                            break;
                        }
                    }
                    None => break,
                }
            }
            // Drain any remaining audio.
            while let Ok(bytes) = audio_rx.try_recv() {
                if write_stop.load(Ordering::SeqCst) {
                    break;
                }
                let msg = RealtimeInput {
                    realtime_input: RealtimeInputAudio {
                        audio: AudioData {
                            data: STANDARD.encode(&bytes),
                            mime_type: "audio/pcm;rate=16000".to_string(),
                        },
                    },
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = ws_sink.send(Message::Text(json)).await;
                }
            }
            // Graceful close.
            let _ = ws_sink.close().await;
            log::info!("[gemini-live] writer closed");
        };

        let read_stop = stop.clone();
        let reader = async {
            while !read_stop.load(Ordering::SeqCst) {
                match ws_stream.next().await {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(e) = Self::handle_server_message(
                            &text,
                            &output_buffer,
                            gemini_output_rate,
                            output_sample_rate,
                            output_channels,
                        ) {
                            log::warn!("[gemini-live] message handling failed: {e}");
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        log::info!("[gemini-live] server closed connection");
                        break;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        log::warn!("[gemini-live] websocket error: {e}");
                        break;
                    }
                    None => {
                        log::info!("[gemini-live] stream ended");
                        break;
                    }
                }
            }
            log::info!("[gemini-live] reader stopped");
        };

        tokio::select! {
            _ = writer => {}
            _ = reader => {}
        }

        log::info!("[gemini-live] run complete");
        Ok(())
    }

    fn resolve_api_key(&self) -> Result<String> {
        let key = if !self.config.api_key.trim().is_empty() {
            self.config.api_key.trim().to_string()
        } else {
            std::env::var("GEMINI_API_KEY").unwrap_or_default()
        };
        if key.is_empty() {
            Err(KoeError::Config(
                "Gemini Live API key not configured. Set in config.yaml or GEMINI_API_KEY env."
                    .into(),
            ))
        } else {
            Ok(key)
        }
    }

    fn resolve_target_language(&self) -> String {
        let code = self.config.target_language_code.trim();
        if code.is_empty() {
            "en".to_string()
        } else {
            code.to_string()
        }
    }

    fn build_setup_message(&self, target_language: &str) -> SetupMessage {
        let mut input_transcription = None;
        let mut output_transcription = None;
        if self.config.input_audio_transcription {
            input_transcription = Some(serde_json::Value::Object(Default::default()));
        }
        if self.config.output_audio_transcription {
            output_transcription = Some(serde_json::Value::Object(Default::default()));
        }
        SetupMessage {
            setup: SetupConfig {
                model: format!("models/{}", self.config.model),
                generation_config: GenerationConfig {
                    response_modalities: vec!["AUDIO".to_string()],
                    input_audio_transcription: input_transcription,
                    output_audio_transcription: output_transcription,
                    translation_config: TranslationConfigGemini {
                        target_language_code: target_language.to_string(),
                        echo_target_language: self.config.echo_target_language,
                    },
                },
            },
        }
    }

    fn handle_server_message(
        text: &str,
        output_buffer: &SharedOutputBuffer,
        gemini_output_rate: u32,
        output_sample_rate: u32,
        output_channels: u16,
    ) -> Result<()> {
        let resp: ServerResponse = serde_json::from_str(text)
            .map_err(|e| KoeError::LlmFailed(format!("parse server message: {e}")))?;

        if let Some(content) = resp.server_content {
            if let Some(trans) = content.input_transcription {
                log::info!(
                    "[gemini-live] input: {} ({})",
                    trans.text,
                    trans.language_code.as_deref().unwrap_or("?")
                );
            }
            if let Some(trans) = content.output_transcription {
                log::info!(
                    "[gemini-live] output: {} ({})",
                    trans.text,
                    trans.language_code.as_deref().unwrap_or("?")
                );
            }
            if let Some(turn) = content.model_turn {
                for part in turn.parts {
                    if let Some(data) = part.inline_data {
                        Self::write_audio_chunk(
                            &data,
                            output_buffer,
                            gemini_output_rate,
                            output_sample_rate,
                            output_channels,
                        )?;
                    }
                }
            }
        }

        if let Some(error) = resp.error {
            log::warn!("[gemini-live] server error: {error:?}");
        }

        Ok(())
    }

    fn write_audio_chunk(
        data: &InlineData,
        output_buffer: &SharedOutputBuffer,
        gemini_output_rate: u32,
        output_sample_rate: u32,
        output_channels: u16,
    ) -> Result<()> {
        if let Some(ref mime) = data.mime_type {
            if !mime.starts_with("audio/pcm") {
                log::warn!("[gemini-live] unexpected audio mime_type: {mime}");
            }
        }

        let decoded = STANDARD
            .decode(&data.data)
            .map_err(|e| KoeError::LlmFailed(format!("base64 decode audio: {e}")))?;

        let chunks = decoded.chunks_exact(2);
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            log::warn!(
                "[gemini-live] audio payload has odd byte count ({}); last byte dropped",
                decoded.len()
            );
        }

        // Parse PCM 16-bit LE samples.
        let mono_samples: Vec<f32> = chunks
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();

        if mono_samples.is_empty() {
            return Ok(());
        }

        // Resample to output rate.
        let resampled = if output_sample_rate != gemini_output_rate {
            resample_linear(&mono_samples, gemini_output_rate, output_sample_rate)
        } else {
            mono_samples
        };

        // Upmix to output channels.
        let channel_count = usize::from(output_channels);
        let mut frame_data = Vec::with_capacity(resampled.len().saturating_mul(channel_count));
        for sample in &resampled {
            for _ in 0..channel_count {
                frame_data.push(*sample);
            }
        }

        let frame = AudioFrame {
            timestamp_ns: now_timestamp_ns(),
            sample_rate: output_sample_rate,
            channels: output_channels,
            data: frame_data,
        };

        output_buffer.write_frame(&frame)?;
        Ok(())
    }
}

// ─── Protocol Types ─────────────────────────────────────────────────

#[derive(Serialize)]
struct SetupMessage {
    setup: SetupConfig,
}

#[derive(Serialize)]
struct SetupConfig {
    model: String,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct GenerationConfig {
    #[serde(rename = "responseModalities")]
    response_modalities: Vec<String>,
    #[serde(rename = "inputAudioTranscription", skip_serializing_if = "Option::is_none")]
    input_audio_transcription: Option<serde_json::Value>,
    #[serde(rename = "outputAudioTranscription", skip_serializing_if = "Option::is_none")]
    output_audio_transcription: Option<serde_json::Value>,
    #[serde(rename = "translationConfig")]
    translation_config: TranslationConfigGemini,
}

#[derive(Serialize)]
struct TranslationConfigGemini {
    #[serde(rename = "targetLanguageCode")]
    target_language_code: String,
    #[serde(rename = "echoTargetLanguage")]
    echo_target_language: bool,
}

#[derive(Serialize)]
struct RealtimeInput {
    #[serde(rename = "realtimeInput")]
    realtime_input: RealtimeInputAudio,
}

#[derive(Serialize)]
struct RealtimeInputAudio {
    audio: AudioData,
}

#[derive(Serialize)]
struct AudioData {
    data: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
}

#[derive(Deserialize, Debug)]
struct ServerResponse {
    #[serde(rename = "serverContent")]
    server_content: Option<ServerContent>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct ServerContent {
    #[serde(rename = "inputTranscription")]
    input_transcription: Option<Transcription>,
    #[serde(rename = "outputTranscription")]
    output_transcription: Option<Transcription>,
    #[serde(rename = "modelTurn")]
    model_turn: Option<ModelTurn>,
}

#[derive(Deserialize, Debug)]
struct Transcription {
    text: String,
    #[serde(rename = "languageCode")]
    language_code: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ModelTurn {
    parts: Vec<TurnPart>,
}

#[derive(Deserialize, Debug)]
struct TurnPart {
    #[serde(rename = "inlineData")]
    inline_data: Option<InlineData>,
}

#[derive(Deserialize, Debug)]
struct InlineData {
    data: String,
    #[serde(rename = "mimeType")]
    #[allow(dead_code)]
    mime_type: Option<String>,
}
