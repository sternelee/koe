use std::collections::VecDeque;

/// Simple energy-threshold VAD for segmenting continuous audio into utterances.
///
/// Operates on i16 PCM 16 kHz mono samples.  States:
///   * `Idle`      – waiting for speech.
///   * `Speaking`  – speech detected, accumulating samples.
///   * `Trailing`  – silence gap after speech; if it exceeds `silence_ms` the
///     segment is finalised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Idle,
    Speaking,
    Trailing,
}

/// A completed speech segment returned by the VAD.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// PCM i16 mono samples @ 16 kHz.
    pub samples: Vec<i16>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

pub struct EnergyVad {
    threshold: f32,
    min_speech_ms: u64,
    silence_ms: u64,
    max_speech_ms: u64,
    sample_rate: u32,
    state: VadState,
    /// Accumulated samples for the current segment.
    buffer: Vec<i16>,
    /// Ring buffer of frame energies for trailing-silence detection.
    energy_history: VecDeque<bool>,
    /// Total samples accumulated in the current segment.
    total_samples: usize,
    /// Consecutive silent frames.
    silent_frames: usize,
    /// Frame size in samples (e.g. 512 for 16kHz ≈ 32ms).
    frame_size: usize,
}

impl EnergyVad {
    pub fn new(
        threshold: f32,
        min_speech_ms: u64,
        silence_ms: u64,
        max_speech_ms: u64,
        sample_rate: u32,
    ) -> Self {
        let frame_size = (sample_rate as usize) / 50; // 20ms frames
        Self {
            threshold,
            min_speech_ms,
            silence_ms,
            max_speech_ms,
            sample_rate,
            state: VadState::Idle,
            buffer: Vec::new(),
            energy_history: VecDeque::new(),
            total_samples: 0,
            silent_frames: 0,
            frame_size,
        }
    }

    /// Push a chunk of i16 mono PCM samples.
    ///
    /// Returns all `SpeechSegment`s finalised during this chunk.  Multiple
    /// segments can be produced when a long utterance hits `max_speech_ms`
    /// mid-chunk.
    pub fn push_samples(&mut self, samples: &[i16]) -> Vec<SpeechSegment> {
        let mut result = Vec::new();
        let mut offset = 0;

        // 1. Fill any partial frame left over from the previous call.
        let partial = self.buffer.len() % self.frame_size;
        if partial > 0 {
            let needed = self.frame_size - partial;
            let take = needed.min(samples.len());
            self.buffer.extend_from_slice(&samples[..take]);
            self.total_samples += take;
            offset = take;
            if self.buffer.len() % self.frame_size == 0 {
                if let Some(segment) = self.process_frame() {
                    result.push(segment);
                }
            }
        }

        // 2. Process complete frames in bulk.
        while offset + self.frame_size <= samples.len() {
            let frame = &samples[offset..offset + self.frame_size];
            self.buffer.extend_from_slice(frame);
            self.total_samples += self.frame_size;
            offset += self.frame_size;
            if let Some(segment) = self.process_frame() {
                result.push(segment);
            }
        }

        // 3. Append remaining partial samples.
        if offset < samples.len() {
            self.buffer.extend_from_slice(&samples[offset..]);
            self.total_samples += samples.len() - offset;
        }

        result
    }

    /// Force-flush any pending audio as a final segment.
    ///
    /// Returns `None` if the VAD never entered the `Speaking` state (i.e. no
    /// speech was detected).
    pub fn flush(&mut self) -> Option<SpeechSegment> {
        if self.total_samples == 0 || self.state == VadState::Idle {
            self.buffer.clear();
            self.total_samples = 0;
            self.silent_frames = 0;
            self.energy_history.clear();
            self.state = VadState::Idle;
            return None;
        }
        let duration_ms = (self.total_samples as u64 * 1000) / (self.sample_rate as u64);
        let samples = std::mem::take(&mut self.buffer);
        self.total_samples = 0;
        self.silent_frames = 0;
        self.energy_history.clear();
        self.state = VadState::Idle;

        if duration_ms >= self.min_speech_ms {
            Some(SpeechSegment { samples, duration_ms })
        } else {
            None
        }
    }

    fn process_frame(&mut self) -> Option<SpeechSegment> {
        let start = self.buffer.len().saturating_sub(self.frame_size);
        let frame = &self.buffer[start..];
        let energy = rms_energy(frame);
        let is_speech = energy >= self.threshold;
        self.energy_history.push_back(is_speech);

        // Keep a rolling window of ~1 second of energy history.
        let history_limit = (self.sample_rate as usize) / self.frame_size;
        while self.energy_history.len() > history_limit {
            self.energy_history.pop_front();
        }

        match self.state {
            VadState::Idle => {
                if is_speech {
                    self.state = VadState::Speaking;
                    self.silent_frames = 0;
                }
                None
            }
            VadState::Speaking => {
                if !is_speech {
                    self.silent_frames += 1;
                    let silent_ms = (self.silent_frames as u64 * 1000)
                        / (self.sample_rate as u64 / self.frame_size as u64);
                    if silent_ms >= self.silence_ms {
                        return self.finalise_segment();
                    }
                } else {
                    self.silent_frames = 0;
                }

                let duration_ms =
                    (self.total_samples as u64 * 1000) / (self.sample_rate as u64);
                if duration_ms >= self.max_speech_ms {
                    return self.finalise_segment();
                }
                None
            }
            VadState::Trailing => {
                // Should not happen; treat as idle.
                self.state = VadState::Idle;
                None
            }
        }
    }

    fn finalise_segment(&mut self) -> Option<SpeechSegment> {
        let duration_ms = (self.total_samples as u64 * 1000) / (self.sample_rate as u64);
        let samples = std::mem::take(&mut self.buffer);
        self.total_samples = 0;
        self.silent_frames = 0;
        self.energy_history.clear();
        self.state = VadState::Idle;

        if duration_ms >= self.min_speech_ms {
            Some(SpeechSegment { samples, duration_ms })
        } else {
            None
        }
    }
}

/// Compute RMS energy of an i16 frame, normalised to 0.0–1.0.
fn rms_energy(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_squares: f64 = frame.iter().map(|s| {
        let f = f64::from(*s) / 32768.0;
        f * f
    }).sum();
    ((sum_squares / frame.len() as f64).sqrt() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_tone(sample_rate: u32, duration_ms: u64, amplitude: f32) -> Vec<i16> {
        let samples = (sample_rate as usize * duration_ms as usize) / 1000;
        (0..samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                let freq = 440.0;
                (amplitude * (t * freq * 2.0 * std::f32::consts::PI).sin() * 32767.0) as i16
            })
            .collect()
    }

    fn generate_silence(sample_rate: u32, duration_ms: u64) -> Vec<i16> {
        let samples = (sample_rate as usize * duration_ms as usize) / 1000;
        vec![0i16; samples]
    }

    #[test]
    fn detects_short_speech_segment() {
        let mut vad = EnergyVad::new(0.01, 200, 500, 10_000, 16_000);

        // 1 second of speech
        let speech = generate_tone(16_000, 1000, 0.5);
        assert!(vad.push_samples(&speech).is_empty());

        // 1 second of silence → should finalise
        let silence = generate_silence(16_000, 1000);
        let segs = vad.push_samples(&silence);
        assert!(!segs.is_empty(), "expected segment after silence gap");
        assert!(segs[0].duration_ms >= 1000, "segment should be ~1s");
    }

    #[test]
    fn ignores_noise_below_threshold() {
        let mut vad = EnergyVad::new(0.1, 200, 500, 10_000, 16_000);

        // Very quiet audio
        let quiet = generate_tone(16_000, 500, 0.01);
        assert!(vad.push_samples(&quiet).is_empty());

        let seg = vad.flush();
        assert!(seg.is_none(), "quiet audio should not produce segment");
    }
}
