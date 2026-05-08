use crate::errors::{KoeError, Result};
use crate::translation::config::TranslationConfig;
use crate::translation::mt::MtClient;
use crate::translation::output_bridge::{AudioFrame, SharedOutputBuffer};
use crate::translation::tts::TtsClient;
use crate::translation::vad::{EnergyVad, SpeechSegment};
use koe_asr::{AsrConfig, AsrEvent, AsrProvider};
use reqwest::Client;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

/// Factory closure that creates a fresh `(AsrConfig, Box<dyn AsrProvider>)` pair.
/// A new provider is needed for every utterance because ASR sessions are
/// single-use (connect → send_audio → finish_input → close).
pub type AsrFactory = Arc<dyn Fn() -> (AsrConfig, Box<dyn AsrProvider>) + Send + Sync>;

/// Real-time translation engine: continuously consumes microphone audio,
/// segments it with VAD, and for each utterance runs ASR → MT → TTS,
/// writing the synthesized audio into the shared mmap output buffer so the
/// HAL virtual-mic plug-in can read it.
pub struct TranslationEngine {
    config: TranslationConfig,
    http_client: Client,
    asr_factory: AsrFactory,
}

impl TranslationEngine {
    pub fn new(config: TranslationConfig, http_client: Client, asr_factory: AsrFactory) -> Self {
        Self {
            config,
            http_client,
            asr_factory,
        }
    }

    /// Main loop.  Runs until `stop` is set to `true`.
    ///
    /// `audio_rx` delivers PCM 16-bit little-endian mono bytes at 16 kHz
    /// (the same format the normal Koe audio pipeline uses).
    pub async fn run(
        &self,
        mut audio_rx: mpsc::Receiver<Vec<u8>>,
        stop: Arc<AtomicBool>,
    ) -> Result<()> {
        let mut vad = EnergyVad::new(
            self.config.vad_energy_threshold,
            self.config.min_speech_ms,
            self.config.silence_ms,
            self.config.max_speech_ms,
            16_000,
        );

        let output_buffer = Arc::new(SharedOutputBuffer::new(
            self.config.output_buffer_frames,
            self.config.output_channels,
            self.config.output_sample_rate,
        )?);

        let mt = Arc::new(MtClient::new(self.http_client.clone(), self.config.mt.clone()));
        let tts = Arc::new(TtsClient::new(self.http_client.clone(), self.config.tts.clone()));

        let asr_factory = self.asr_factory.clone();
        let source_language = self.config.source_language.clone();
        let target_language = self.config.target_language.clone();
        let mt_enabled = self.config.mt.enabled;
        let tts_enabled = self.config.tts.enabled;
        let output_sample_rate = self.config.output_sample_rate;
        let output_channels = self.config.output_channels;

        // Bounded queue preserves utterance order and prevents unbounded memory growth.
        let (segment_tx, mut segment_rx) = mpsc::channel::<SpeechSegment>(4);

        // Spawn a single worker that processes segments serially.
        let processor_handle = tokio::spawn(async move {
            while let Some(segment) = segment_rx.recv().await {
                run_segment_pipeline(
                    segment,
                    asr_factory.clone(),
                    mt.clone(),
                    tts.clone(),
                    output_buffer.clone(),
                    source_language.clone(),
                    target_language.clone(),
                    mt_enabled,
                    tts_enabled,
                    output_sample_rate,
                    output_channels,
                )
                .await;
            }
            log::info!("[translation] segment processor stopped");
        });

        while !stop.load(Ordering::SeqCst) {
            tokio::select! {
                Some(bytes) = audio_rx.recv() => {
                    let samples: Vec<i16> = bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    for segment in vad.push_samples(&samples) {
                        if segment_tx.try_send(segment).is_err() {
                            log::warn!("[translation] segment queue full, dropping segment");
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if let Some(segment) = vad.flush() {
                        if segment_tx.try_send(segment).is_err() {
                            log::warn!("[translation] segment queue full, dropping segment");
                        }
                    }
                }
            }
        }

        // Final flush when stopping.
        if let Some(segment) = vad.flush() {
            if segment_tx.try_send(segment).is_err() {
                // Queue full — process inline so the last utterance is not lost.
                // At this point the Arc clones owned by the processor are still alive,
                // but we cannot await the processor because it may be blocked on the
                // same full queue.  We therefore skip this final segment; the
                // processor will drain the queued ones.
                log::warn!("[translation] final segment dropped because queue is full");
            }
        }
        drop(segment_tx);
        if let Err(e) = processor_handle.await {
            log::warn!("[translation] segment processor task failed: {e}");
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_segment_pipeline(
    segment: SpeechSegment,
    asr_factory: AsrFactory,
    mt: Arc<MtClient>,
    tts: Arc<TtsClient>,
    output_buffer: Arc<SharedOutputBuffer>,
    source_language: String,
    target_language: String,
    mt_enabled: bool,
    tts_enabled: bool,
    output_sample_rate: u32,
    output_channels: u16,
) {
    // 1. ASR
    let text = match run_asr(&segment, &asr_factory).await {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => return,
        Err(e) => {
            log::warn!("[translation] ASR failed: {e}");
            return;
        }
    };
    log::info!("[translation] ASR: {text}");

    // 2. MT
    if !mt_enabled {
        return;
    }
    let translated = match mt.translate(&text, &source_language, &target_language).await {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[translation] MT failed: {e}");
            return;
        }
    };
    log::info!("[translation] MT: {translated}");

    if translated.trim().is_empty() {
        return;
    }

    // 3. TTS
    if !tts_enabled {
        return;
    }
    let (samples, tts_rate) = match tts.synthesize(&translated).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[translation] TTS failed: {e}");
            return;
        }
    };

    // 4. Resample to output sample rate
    let output_samples = if tts_rate != output_sample_rate {
        resample_linear(&samples, tts_rate, output_sample_rate)
    } else {
        samples
    };

    // 5. Write to shared mmap buffer
    let frame = AudioFrame {
        timestamp_ns: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
        sample_rate: output_sample_rate,
        channels: output_channels,
        data: output_samples,
    };

    if let Err(e) = output_buffer.write_frame(&frame) {
        log::warn!("[translation] output buffer write failed: {e}");
    }
}

async fn run_asr(
    segment: &SpeechSegment,
    asr_factory: &AsrFactory,
) -> Result<String> {
    let (asr_config, mut asr) = (asr_factory)();

    asr.connect(&asr_config)
        .await
        .map_err(|e| KoeError::LlmFailed(format!("ASR connect failed: {e}")))?;

    // Stream audio in ~20 ms chunks (320 samples = 640 bytes at 16 kHz).
    let bytes: Vec<u8> = segment.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let chunk_bytes = 640;
    for chunk in bytes.chunks(chunk_bytes) {
        if let Err(e) = asr.send_audio(chunk).await {
            if timeout(Duration::from_secs(5), asr.close()).await.is_err() {
                log::warn!("[translation] ASR close timed out after send failure");
            }
            return Err(KoeError::LlmFailed(format!("ASR send failed: {e}")));
        }
    }

    if let Err(e) = asr.finish_input().await {
        if timeout(Duration::from_secs(5), asr.close()).await.is_err() {
            log::warn!("[translation] ASR close timed out after finish failure");
        }
        return Err(KoeError::LlmFailed(format!("ASR finish failed: {e}")));
    }

    let mut text = String::new();
    let wait_result = timeout(
        Duration::from_millis(asr_config.final_wait_timeout_ms),
        async {
            loop {
                match asr.next_event().await {
                    Ok(AsrEvent::Final(t)) => {
                        text = t;
                        break Ok(());
                    }
                    Ok(AsrEvent::Closed) => break Ok(()),
                    Ok(AsrEvent::Error(msg)) => {
                        break Err(KoeError::LlmFailed(format!("ASR error: {msg}")));
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        break Err(KoeError::LlmFailed(format!("ASR event read failed: {e}")));
                    }
                }
            }
        },
    )
    .await;

    if timeout(Duration::from_secs(5), asr.close()).await.is_err() {
        log::warn!("[translation] ASR close timed out after event loop");
    }

    match wait_result {
        Ok(Ok(())) => Ok(text),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Timeout — return whatever text we managed to collect (may be empty).
            Ok(text)
        }
    }
}

/// Simple linear-interpolation resampler.
/// Good enough for speech where quality is secondary to latency.
fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let output_len = ((input.len() - 1) as f64 * ratio).ceil() as usize + 1;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let src_idx = (src_pos as usize).min(input.len().saturating_sub(1));
        let frac = (src_pos - src_idx as f64) as f32;
        let s0 = input[src_idx];
        let s1 = input.get(src_idx + 1).copied().unwrap_or(s0);
        output.push(s0 + (s1 - s0) * frac);
    }
    output
}
