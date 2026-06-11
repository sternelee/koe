use crate::errors::{KoeError, Result};
use crate::translation::config::TranslationConfig;
use crate::translation::gemini_live::GeminiLiveClient;
use crate::translation::mt::MtClient;
use crate::translation::output_bridge::{AudioFrame, SharedOutputBuffer};
use crate::translation::tts::TtsClient;
use crate::translation::vad::{EnergyVad, SpeechSegment};
use koe_asr::{AsrConfig, AsrEvent, AsrProvider};
use reqwest::Client;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration, Instant};

/// Factory closure that creates a fresh `(AsrConfig, Box<dyn AsrProvider>)` pair.
/// A new provider is needed for every utterance because ASR sessions are
/// single-use (connect → send_audio → finish_input → close).
pub type AsrFactory = Arc<dyn Fn() -> (AsrConfig, Box<dyn AsrProvider>) + Send + Sync>;

/// Real-time translation engine. Depending on runtime readiness it either:
///
/// - runs the full ASR → MT → TTS pipeline and writes synthesized audio to the
///   shared mmap output buffer, or
/// - falls back to microphone passthrough so the virtual mic still behaves like
///   a regular input device when translation backends are not configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationMode {
    Translate,
    Passthrough,
}

pub struct TranslationEngine {
    config: TranslationConfig,
    http_client: Client,
    asr_factory: AsrFactory,
    mode: TranslationMode,
}

impl TranslationEngine {
    pub fn new(
        config: TranslationConfig,
        http_client: Client,
        asr_factory: AsrFactory,
        mode: TranslationMode,
    ) -> Self {
        Self {
            config,
            http_client,
            asr_factory,
            mode,
        }
    }

    /// Main loop. Runs until `stop` is set to `true`.
    ///
    /// `audio_rx` delivers PCM 16-bit little-endian mono bytes at 16 kHz
    /// (the same format the normal Koe audio pipeline uses).
    pub async fn run(
        &self,
        mut audio_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        stop: Arc<AtomicBool>,
    ) -> Result<()> {
        let output_buffer = Arc::new(SharedOutputBuffer::new(
            self.config.output_buffer_frames,
            self.config.output_channels,
            self.config.output_sample_rate,
        )?);
        log::info!(
            "[translation] output buffer ready frames={} rate={} channels={} mode={:?}",
            self.config.output_buffer_frames,
            self.config.output_sample_rate,
            self.config.output_channels,
            self.mode,
        );

        if self.mode == TranslationMode::Passthrough {
            log::info!("[translation] backend incomplete; using passthrough virtual mic mode");
            return run_passthrough_loop(
                &mut audio_rx,
                stop,
                output_buffer,
                self.config.output_sample_rate,
                self.config.output_channels,
            )
            .await;
        }

        // Gemini Live Translate: bidirectional streaming, bypasses segment pipeline.
        if self.config.gemini_live.enabled {
            log::info!("[translation] using Gemini Live Translate");
            let client = GeminiLiveClient::new(self.config.gemini_live.clone());
            return client
                .run(
                    audio_rx,
                    output_buffer,
                    stop,
                    self.config.output_sample_rate,
                    self.config.output_channels,
                )
                .await;
        }

        let mut vad = EnergyVad::new(
            self.config.vad_energy_threshold,
            self.config.min_speech_ms,
            self.config.silence_ms,
            self.config.max_speech_ms,
            16_000,
        );

        let source_language = self.config.source_language.clone();
        let target_language = self.config.target_language.clone();
        let mt = Arc::new(MtClient::new(
            self.http_client.clone(),
            self.config.mt.clone(),
            Some(source_language.as_str()),
        ));
        let tts = Arc::new(TtsClient::new(
            self.http_client.clone(),
            self.config.tts.clone(),
        ));

        let asr_factory = self.asr_factory.clone();
        let mt_enabled = self.config.mt.enabled;
        let tts_enabled = self.config.tts.enabled;
        let output_sample_rate = self.config.output_sample_rate;
        let output_channels = self.config.output_channels;

        // Bounded so a slow MT/TTS stage applies backpressure instead of letting
        // unbounded audio segments accumulate. Live segments are dropped (with a
        // warning) when the backlog is full — for a real-time translator a stale
        // backlog is useless — while the final flush below uses a blocking send so
        // the last utterance is never lost on shutdown.
        let (segment_tx, mut segment_rx) = mpsc::channel::<SpeechSegment>(8);

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
        let mut last_audio_at = None;
        let mut received_chunks = 0u64;
        let mut received_bytes = 0u64;
        let mut last_rx_log_at: Option<Instant> = None;

        while !stop.load(Ordering::SeqCst) {
            tokio::select! {
                Some(bytes) = audio_rx.recv() => {
                    last_audio_at = Some(Instant::now());
                    let samples: Vec<i16> = bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    received_chunks = received_chunks.saturating_add(1);
                    received_bytes = received_bytes.saturating_add(bytes.len() as u64);
                    let now = Instant::now();
                    let should_log = last_rx_log_at
                        .map(|last| now.duration_since(last) >= Duration::from_secs(2))
                        .unwrap_or(true);
                    if should_log {
                        log::info!(
                            "[translation] engine received audio chunks={received_chunks} bytes={received_bytes} last_samples={} peak={:.4}",
                            samples.len(),
                            pcm16_peak(&samples),
                        );
                        last_rx_log_at = Some(now);
                    }
                    for segment in vad.push_samples(&samples) {
                        log::info!(
                            "[translation] VAD segment duration={}ms samples={}",
                            segment.duration_ms,
                            segment.samples.len(),
                        );
                        match segment_tx.try_send(segment) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                log::warn!("[translation] segment backlog full; dropping utterance to bound memory");
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                log::warn!("[translation] segment processor stopped before utterance handoff");
                                break;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if let Some(idle_for) = last_audio_at.map(|last| last.elapsed()) {
                        let idle_ms = idle_for.as_millis().min(u128::from(u64::MAX)) as u64;
                        if idle_ms >= self.config.silence_ms {
                            last_audio_at = None;
                            if let Some(segment) = vad.flush_if_inactive(idle_ms) {
                                log::info!(
                                    "[translation] VAD idle flush duration={}ms samples={}",
                                    segment.duration_ms,
                                    segment.samples.len(),
                                );
                                match segment_tx.try_send(segment) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        log::warn!("[translation] segment backlog full; dropping idle-flush utterance");
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {
                                        log::warn!("[translation] segment processor stopped before idle flush");
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(segment) = vad.flush() {
            log::info!(
                "[translation] VAD final flush duration={}ms samples={}",
                segment.duration_ms,
                segment.samples.len(),
            );
            if segment_tx.send(segment).await.is_err() {
                log::warn!("[translation] segment processor stopped before final flush");
            }
        }
        drop(segment_tx);
        if let Err(e) = processor_handle.await {
            log::warn!("[translation] segment processor task failed: {e}");
        }

        Ok(())
    }
}

async fn run_passthrough_loop(
    audio_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    output_buffer: Arc<SharedOutputBuffer>,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<()> {
    while !stop.load(Ordering::SeqCst) {
        tokio::select! {
            Some(bytes) = audio_rx.recv() => {
                write_passthrough_chunk(&bytes, &output_buffer, output_sample_rate, output_channels)?;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    while let Ok(bytes) = audio_rx.try_recv() {
        write_passthrough_chunk(&bytes, &output_buffer, output_sample_rate, output_channels)?;
    }

    Ok(())
}

fn write_passthrough_chunk(
    bytes: &[u8],
    output_buffer: &SharedOutputBuffer,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<()> {
    let mono_samples: Vec<f32> = bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
        .collect();
    write_passthrough_mono_samples(
        &mono_samples,
        16_000,
        output_buffer,
        output_sample_rate,
        output_channels,
    )
}

#[cfg(test)]
fn write_passthrough_segment(
    segment: &SpeechSegment,
    output_buffer: &SharedOutputBuffer,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<()> {
    let mono_samples: Vec<f32> = segment
        .samples
        .iter()
        .map(|sample| *sample as f32 / 32768.0)
        .collect();
    write_passthrough_mono_samples(
        &mono_samples,
        16_000,
        output_buffer,
        output_sample_rate,
        output_channels,
    )
}

fn write_passthrough_mono_samples(
    mono_samples: &[f32],
    input_sample_rate: u32,
    output_buffer: &SharedOutputBuffer,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<()> {
    if mono_samples.is_empty() {
        return Ok(());
    }

    let resampled = if output_sample_rate != input_sample_rate {
        resample_linear(mono_samples, input_sample_rate, output_sample_rate)
    } else {
        mono_samples.to_vec()
    };

    let data = upmix_mono(&resampled, output_channels);
    let frame = AudioFrame {
        timestamp_ns: now_timestamp_ns(),
        sample_rate: output_sample_rate,
        channels: output_channels,
        data,
    };

    output_buffer.write_frame(&frame)
}

pub(crate) fn now_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn upmix_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }

    let channels = usize::from(channels);
    let mut out = Vec::with_capacity(samples.len().saturating_mul(channels));
    for sample in samples {
        for _ in 0..channels {
            out.push(*sample);
        }
    }
    out
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
    log::info!(
        "[translation] processing segment duration={}ms samples={}",
        segment.duration_ms,
        segment.samples.len(),
    );
    // 1. ASR
    let text = match run_asr(&segment, &asr_factory).await {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => {
            log::warn!("[translation] ASR produced no text; dropping untranslated segment");
            return;
        }
        Err(e) => {
            log::warn!("[translation] ASR failed: {e}; dropping untranslated segment");
            return;
        }
    };
    log::info!("[translation] ASR: {text}");

    // 2. MT
    if !mt_enabled {
        log::warn!(
            "[translation] MT disabled while translate mode is active; dropping untranslated segment"
        );
        return;
    }
    let translated = match mt
        .translate(&text, &source_language, &target_language)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[translation] MT failed: {e}; dropping untranslated segment");
            return;
        }
    };
    log::info!("[translation] MT: {translated}");

    if translated.trim().is_empty() {
        log::warn!("[translation] MT returned empty text; dropping untranslated segment");
        return;
    }

    // 3. TTS
    if !tts_enabled {
        log::warn!(
            "[translation] TTS disabled while translate mode is active; dropping untranslated segment"
        );
        return;
    }
    let (samples, tts_rate) = match tts.synthesize(&translated, Some(&target_language)).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[translation] TTS failed: {e}; dropping untranslated segment");
            return;
        }
    };
    log::info!(
        "[translation] TTS produced samples={} rate={tts_rate}",
        samples.len(),
    );

    // 4. Resample to output sample rate
    let output_samples = if tts_rate != output_sample_rate {
        resample_linear(&samples, tts_rate, output_sample_rate)
    } else {
        samples
    };
    let output_sample_count = output_samples.len();
    let output_frames = output_sample_count / usize::from(output_channels.max(1));

    // 5. Write to shared mmap buffer
    let frame = AudioFrame {
        timestamp_ns: now_timestamp_ns(),
        sample_rate: output_sample_rate,
        channels: output_channels,
        data: output_samples,
    };

    if let Err(e) = output_buffer.write_frame(&frame) {
        log::warn!("[translation] output buffer write failed: {e}");
    } else {
        log::info!(
            "[translation] output buffer wrote frames={output_frames} samples={output_sample_count} rate={output_sample_rate} channels={output_channels}",
        );
    }
}

fn pcm16_peak(samples: &[i16]) -> f32 {
    let peak = samples
        .iter()
        .map(|sample| i32::from(*sample).unsigned_abs().min(32768))
        .max()
        .unwrap_or(0);
    peak as f32 / 32768.0
}

async fn run_asr(segment: &SpeechSegment, asr_factory: &AsrFactory) -> Result<String> {
    let (asr_config, mut asr) = (asr_factory)();

    asr.connect(&asr_config)
        .await
        .map_err(|e| KoeError::LlmFailed(format!("ASR connect failed: {e}")))?;

    // Stream audio in ~20 ms chunks (320 samples = 640 bytes at 16 kHz).
    let bytes: Vec<u8> = segment
        .samples
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
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
                    Ok(AsrEvent::Closed(_)) => break Ok(()),
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
pub(crate) fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn shared_buffer_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn passthrough_segment_writes_audio_when_translation_fails() {
        let _guard = shared_buffer_test_lock().lock().unwrap();
        let buffer = SharedOutputBuffer::new(256, 1, 48_000).expect("buffer");
        let segment = SpeechSegment {
            samples: vec![1200, -1200, 2400, -2400, 1200, -1200, 0, 0],
            duration_ms: 1,
        };

        write_passthrough_segment(&segment, &buffer, 48_000, 1).expect("fallback write");

        let snapshot = buffer.snapshot();
        assert!(snapshot.header.write_index_frames > 0);
        assert!(snapshot.header.last_timestamp_ns > 0);
        assert!(snapshot.samples.iter().any(|sample| sample.abs() > 0.0));
    }
}
