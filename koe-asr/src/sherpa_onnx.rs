use crate::config::AsrConfig;
use crate::error::Result;
use crate::event::AsrEvent;
use crate::provider::AsrProvider;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineWhisperModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::sync::{mpsc, Mutex};

const SAMPLE_RATE: i32 = 16000;
const VAD_WINDOW_SIZE: usize = 512;

pub struct SherpaOnnxProvider {
    is_closed: Arc<AtomicBool>,
    #[allow(dead_code)]
    result_sender: mpsc::Sender<AsrEvent>,
    result_receiver: Arc<Mutex<Option<mpsc::Receiver<AsrEvent>>>>,
    audio_sender: std::sync::mpsc::Sender<Vec<f32>>,
}

#[derive(Clone, Debug)]
pub enum ModelType {
    SenseVoice,
    Whisper,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamingMode {
    Vad,
    Interval,
}

#[derive(Clone, Debug)]
pub struct VadParams {
    pub threshold: f32,
    pub min_speech_duration: f32,
    pub min_silence_duration: f32,
    pub max_speech_duration: f32,
}

impl Default for VadParams {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_speech_duration: 0.25,
            min_silence_duration: 0.5,
            max_speech_duration: 30.0,
        }
    }
}

impl SherpaOnnxProvider {
    pub fn new(
        model_type: ModelType,
        model_dir: PathBuf,
        streaming_mode: StreamingMode,
        vad_params: VadParams,
    ) -> Self {
        let (result_sender, result_receiver) = mpsc::channel(100);
        let (audio_sender, audio_receiver) = std::sync::mpsc::channel::<Vec<f32>>();

        let is_closed = Arc::new(AtomicBool::new(false));
        let is_closed_clone = is_closed.clone();
        let result_sender_clone = result_sender.clone();

        thread::spawn(move || {
            let recognizer = match model_type {
                ModelType::SenseVoice => {
                    let model_path = model_dir.join("model.int8.onnx");
                    let tokens_path = model_dir.join("tokens.txt");

                    let mut config = OfflineRecognizerConfig::default();
                    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                        model: Some(model_path.to_string_lossy().to_string()),
                        language: Some("auto".to_string()),
                        use_itn: true,
                    };
                    config.model_config.tokens = Some(tokens_path.to_string_lossy().to_string());
                    config.model_config.provider = Some("cpu".to_string());
                    config.model_config.num_threads = 2;

                    OfflineRecognizer::create(&config).expect("Failed to create SenseVoice")
                }
                ModelType::Whisper => {
                    let encoder_path = model_dir.join("tiny.en-encoder.int8.onnx");
                    let decoder_path = model_dir.join("tiny.en-decoder.int8.onnx");
                    let tokens_path = model_dir.join("tiny.en-tokens.txt");

                    let mut config = OfflineRecognizerConfig::default();
                    config.model_config.whisper = OfflineWhisperModelConfig {
                        encoder: Some(encoder_path.to_string_lossy().to_string()),
                        decoder: Some(decoder_path.to_string_lossy().to_string()),
                        language: Some("en".to_string()),
                        task: Some("transcribe".to_string()),
                        tail_paddings: Default::default(),
                        enable_token_timestamps: Default::default(),
                        enable_segment_timestamps: Default::default(),
                    };
                    config.model_config.tokens = Some(tokens_path.to_string_lossy().to_string());
                    config.model_config.provider = Some("cpu".to_string());
                    config.model_config.num_threads = 2;

                    OfflineRecognizer::create(&config).expect("Failed to create Whisper")
                }
            };

            let vad_model_path = model_dir.join("silero_vad.onnx");
            let mut silero_config = SileroVadModelConfig::default();
            silero_config.model = Some(vad_model_path.to_string_lossy().to_string());
            silero_config.threshold = vad_params.threshold;
            silero_config.min_speech_duration = vad_params.min_speech_duration;
            silero_config.min_silence_duration = vad_params.min_silence_duration;
            silero_config.max_speech_duration = vad_params.max_speech_duration;
            silero_config.window_size = VAD_WINDOW_SIZE as i32;

            let vad_config = VadModelConfig {
                silero_vad: silero_config,
                ten_vad: Default::default(),
                sample_rate: SAMPLE_RATE,
                num_threads: 1,
                provider: Some("cpu".to_string()),
                debug: false,
            };

            let vad =
                VoiceActivityDetector::create(&vad_config, 60.0).expect("Failed to create VAD");

            let mut audio_buffer = Vec::<f32>::new();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            loop {
                if is_closed_clone.load(Ordering::SeqCst) {
                    break;
                }

                while let Ok(samples) = audio_receiver.try_recv() {
                    audio_buffer.extend_from_slice(&samples);
                }

                match streaming_mode {
                    StreamingMode::Vad => {
                        let mut offset = 0;
                        while offset + VAD_WINDOW_SIZE <= audio_buffer.len() {
                            vad.accept_waveform(&audio_buffer[offset..offset + VAD_WINDOW_SIZE]);
                            offset += VAD_WINDOW_SIZE;
                        }

                        while !vad.is_empty() {
                            if let Some(segment) = vad.front() {
                                vad.pop();
                                let stream = recognizer.create_stream();
                                stream.accept_waveform(SAMPLE_RATE, segment.samples());
                                recognizer.decode(&stream);
                                if let Some(result) = stream.get_result() {
                                    let text = result.text.trim().to_string();
                                    if !text.is_empty() {
                                        let sender = result_sender_clone.clone();
                                        let text_clone = text.clone();
                                        rt.spawn(async move {
                                            let _ = sender.send(AsrEvent::Final(text_clone)).await;
                                        });
                                    }
                                }
                            }
                        }
                    }
                    StreamingMode::Interval => {
                        if !audio_buffer.is_empty() {
                            let stream = recognizer.create_stream();
                            stream.accept_waveform(SAMPLE_RATE, &audio_buffer);
                            recognizer.decode(&stream);
                            if let Some(result) = stream.get_result() {
                                let text = result.text.trim().to_string();
                                if !text.is_empty() {
                                    let sender = result_sender_clone.clone();
                                    let text_clone = text.clone();
                                    rt.spawn(async move {
                                        let _ = sender.send(AsrEvent::Final(text_clone)).await;
                                    });
                                }
                            }
                            audio_buffer.clear();
                        }
                    }
                }

                thread::sleep(std::time::Duration::from_millis(50));
            }
        });

        Self {
            is_closed,
            result_sender,
            result_receiver: Arc::new(Mutex::new(Some(result_receiver))),
            audio_sender,
        }
    }

    fn pcm_to_f32(&self, pcm_data: &[u8]) -> Vec<f32> {
        let mut samples = Vec::with_capacity(pcm_data.len() / 2);
        for chunk in pcm_data.chunks(2) {
            if chunk.len() == 2 {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                samples.push(sample as f32 / 32768.0);
            }
        }
        samples
    }
}

impl AsrProvider for SherpaOnnxProvider {
    async fn connect(&mut self, _config: &AsrConfig) -> Result<()> {
        Ok(())
    }

    async fn send_audio(&mut self, frame: &[u8]) -> Result<()> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Ok(());
        }

        let samples = self.pcm_to_f32(frame);
        let _ = self.audio_sender.send(samples);
        Ok(())
    }

    async fn finish_input(&mut self) -> Result<()> {
        if self.is_closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        Ok(())
    }

    async fn next_event(&mut self) -> Result<AsrEvent> {
        let mut receiver_guard = self.result_receiver.lock().await;
        if let Some(ref mut rx) = *receiver_guard {
            match rx.recv().await {
                Some(event) => Ok(event),
                None => Ok(AsrEvent::Closed),
            }
        } else {
            Ok(AsrEvent::Closed)
        }
    }

    async fn close(&mut self) -> Result<()> {
        self.is_closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}
