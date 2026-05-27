use crate::errors::{KoeError, Result};
use crate::translation::config::TtsConfig;
use std::path::Path;

const KITTEN_SAMPLE_RATE: u32 = 24_000;
const KITTEN_MAX_CHUNK_LEN: usize = 400;
const KITTEN_TRAILING_SAMPLES_TO_TRIM: usize = 5_000;
const KITTEN_SYMBOL_PADDING_ID: i64 = 0;
const KITTEN_SYMBOL_EOS_ID: i64 = 10;
const KITTEN_PUNCTUATION: &str = ";:,.!?¡¿—…”«»\"\" ";
const KITTEN_LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const KITTEN_LETTERS_IPA: &str = "ɑɐɒæɓʙβɔɕçɗɖðʤəɘɚɛɜɝɞɟʄɡɠɢʛɦɧħɥʜɨɪʝɭɬɫɮʟɱɯɰŋɳɲɴøɵɸθœɶʘɹɺɾɻʀʁɽʂʃʈʧʉʊʋⱱʌɣɤʍχʎʏʑʐʒʔʡʕʢǀǁǂǃˈˌːˑʼʴʰʱʲʷˠˤ˞↓↑→↗↘'̩'ᵻ";
const KITTEN_DEFAULT_PRESET_VOICE: &str = "Jasper";

const KITTEN_PRESET_VOICES: &[(&str, &str, i32)] = &[
    ("Bella", "expr-voice-2-f", 0),
    ("Jasper", "expr-voice-2-m", 1),
    ("Luna", "expr-voice-3-f", 2),
    ("Bruno", "expr-voice-3-m", 3),
    ("Rosie", "expr-voice-4-f", 4),
    ("Hugo", "expr-voice-4-m", 5),
    ("Kiki", "expr-voice-5-f", 6),
    ("Leo", "expr-voice-5-m", 7),
];

pub(crate) fn kitten_language_code(language: &str) -> Option<&'static str> {
    let normalized = language.trim().to_ascii_lowercase();
    let base = normalized.split(['-', '_']).next().unwrap_or("");
    match base {
        "" | "en" => Some("en"),
        _ => None,
    }
}

pub(crate) fn model_files_ready(model_path: &Path) -> bool {
    model_path.join("config.json").exists()
        && model_path.join("voices.npz").exists()
        && model_path
            .read_dir()
            .ok()
            .into_iter()
            .flat_map(|entries| entries.flatten())
            .any(|entry| {
                entry.path().is_file()
                    && entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("onnx"))
            })
}

pub(crate) fn speaker_id_for_preset_voice(preset_voice: &str) -> Option<i32> {
    KITTEN_PRESET_VOICES
        .iter()
        .find(|(alias, voice_id, _)| {
            alias.eq_ignore_ascii_case(preset_voice) || voice_id.eq_ignore_ascii_case(preset_voice)
        })
        .map(|(_, _, speaker_id)| *speaker_id)
}

pub(crate) fn preset_voice_for_speaker_id(speaker_id: i32) -> &'static str {
    KITTEN_PRESET_VOICES
        .iter()
        .find(|(_, _, id)| *id == speaker_id)
        .map(|(alias, _, _)| *alias)
        .unwrap_or(KITTEN_DEFAULT_PRESET_VOICE)
}

#[cfg(feature = "kitten-onnx")]
mod imp {
    use super::*;
    use misaki_rs::{Language, G2P};
    use ndarray::{Array1, Array2};
    use ndarray_npy::NpzReader;
    use ort::{inputs, session::Session, value::Tensor};
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::fs::File;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct KittenOnnxBackend {
        inner: Arc<KittenOnnxBackendInner>,
    }

    struct KittenOnnxBackendInner {
        session: Mutex<Session>,
        style_embeddings: HashMap<String, Array2<f32>>,
        symbol_ids: HashMap<char, i64>,
        g2p: G2P,
        voice_key: String,
        speed: f32,
    }

    #[derive(Debug, Deserialize)]
    struct KittenModelMetadata {
        #[serde(rename = "type")]
        model_type: String,
        model_file: String,
        voices: String,
        #[serde(default)]
        speed_priors: HashMap<String, f32>,
        #[serde(default)]
        voice_aliases: HashMap<String, String>,
    }

    impl KittenOnnxBackend {
        pub fn new(model_dir: &Path, config: &TtsConfig) -> Result<Self> {
            let metadata = load_model_metadata(model_dir)?;
            if !metadata.model_type.eq_ignore_ascii_case("onnx2") {
                return Err(KoeError::Config(format!(
                    "Kitten ONNX expects ONNX2 config.json, got {}",
                    metadata.model_type
                )));
            }

            let model_path = model_dir.join(&metadata.model_file);
            if !model_path.exists() {
                return Err(KoeError::Config(format!(
                    "Kitten ONNX model file not found: {}",
                    model_path.display()
                )));
            }

            let voices_path = model_dir.join(&metadata.voices);
            if !voices_path.exists() {
                return Err(KoeError::Config(format!(
                    "Kitten ONNX voices file not found: {}",
                    voices_path.display()
                )));
            }

            let (voice_key, speaker_id) = resolve_voice_selection(config, &metadata)?;
            let session = Session::builder()
                .map_err(|e| KoeError::Config(format!("Kitten ONNX session builder: {e}")))?
                .with_intra_threads(2)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX threads: {e}")))?
                .commit_from_file(&model_path)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX load model: {e}")))?;
            let style_embeddings = load_style_embeddings(&voices_path)?;
            if !style_embeddings.contains_key(&voice_key) {
                return Err(KoeError::Config(format!(
                    "Kitten ONNX voice embedding missing for {}",
                    voice_key
                )));
            }
            let speed = config.speed
                * metadata
                    .speed_priors
                    .get(&voice_key)
                    .copied()
                    .unwrap_or(1.0);

            log::info!(
                "[tts] Kitten ONNX backend loaded from {} (voice={}, speaker_id={}, speed={})",
                model_dir.display(),
                voice_key,
                speaker_id,
                speed
            );

            Ok(Self {
                inner: Arc::new(KittenOnnxBackendInner {
                    session: Mutex::new(session),
                    style_embeddings,
                    symbol_ids: build_symbol_id_map(),
                    g2p: G2P::new(Language::EnglishUS),
                    voice_key,
                    speed,
                }),
            })
        }

        pub fn synthesize(&self, text: &str) -> Result<(Vec<f32>, u32)> {
            let chunks = chunk_text(text, KITTEN_MAX_CHUNK_LEN);
            let mut all_samples = Vec::new();

            for chunk in chunks {
                let input_ids = self.phonemize_chunk(&chunk)?;
                if input_ids.is_empty() {
                    continue;
                }
                let style = self.style_for_text(&chunk)?;
                let mut samples = self.run_model(&input_ids, &style)?;
                if samples.len() > KITTEN_TRAILING_SAMPLES_TO_TRIM {
                    samples.truncate(samples.len() - KITTEN_TRAILING_SAMPLES_TO_TRIM);
                }
                all_samples.extend(samples);
            }

            Ok((all_samples, KITTEN_SAMPLE_RATE))
        }

        fn phonemize_chunk(&self, text: &str) -> Result<Vec<i64>> {
            let (phoneme_text, _) = self
                .inner
                .g2p
                .g2p(text)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX phonemize: {e}")))?;
            Ok(encode_phoneme_text(&self.inner.symbol_ids, &phoneme_text))
        }

        fn style_for_text(&self, text: &str) -> Result<Array2<f32>> {
            let style = self
                .inner
                .style_embeddings
                .get(&self.inner.voice_key)
                .ok_or_else(|| {
                    KoeError::Config(format!(
                        "Kitten ONNX voice embedding missing for {}",
                        self.inner.voice_key
                    ))
                })?;
            let rows = style.nrows();
            if rows == 0 || style.ncols() != 256 {
                return Err(KoeError::Config(format!(
                    "Kitten ONNX voice embedding has invalid shape {:?}",
                    style.shape()
                )));
            }
            let ref_id = text.chars().count().min(rows.saturating_sub(1));
            let row = style.row(ref_id).to_owned();
            Array2::from_shape_vec((1, style.ncols()), row.to_vec())
                .map_err(|e| KoeError::Config(format!("Kitten ONNX style tensor shape: {e}")))
        }

        fn run_model(&self, input_ids: &[i64], style: &Array2<f32>) -> Result<Vec<f32>> {
            let input_ids = Array2::from_shape_vec((1, input_ids.len()), input_ids.to_vec())
                .map_err(|e| KoeError::Config(format!("Kitten ONNX input shape: {e}")))?;
            let speed = Array1::from_vec(vec![self.inner.speed]);

            let input_ids_tensor = Tensor::from_array(input_ids)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX input tensor: {e}")))?;
            let style_tensor = Tensor::from_array(style.clone())
                .map_err(|e| KoeError::Config(format!("Kitten ONNX style tensor: {e}")))?;
            let speed_tensor = Tensor::from_array(speed)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX speed tensor: {e}")))?;

            let mut session = self.inner.session.lock().unwrap();
            let outputs = session
                .run(inputs![
                    "input_ids" => input_ids_tensor,
                    "style" => style_tensor,
                    "speed" => speed_tensor
                ])
                .map_err(|e| KoeError::Config(format!("Kitten ONNX run: {e}")))?;
            let (_, waveform) = outputs["waveform"]
                .try_extract_tensor::<f32>()
                .map_err(|e| KoeError::Config(format!("Kitten ONNX waveform: {e}")))?;
            Ok(waveform.to_vec())
        }
    }

    fn load_model_metadata(model_dir: &Path) -> Result<KittenModelMetadata> {
        let config_path = model_dir.join("config.json");
        let bytes = std::fs::read(&config_path)
            .map_err(|e| KoeError::Config(format!("Kitten ONNX read config.json: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| KoeError::Config(format!("Kitten ONNX parse config.json: {e}")))
    }

    fn load_style_embeddings(voices_path: &Path) -> Result<HashMap<String, Array2<f32>>> {
        let file = File::open(voices_path)
            .map_err(|e| KoeError::Config(format!("Kitten ONNX open voices.npz: {e}")))?;
        let mut npz = NpzReader::new(file)
            .map_err(|e| KoeError::Config(format!("Kitten ONNX read voices.npz: {e}")))?;
        let names = npz
            .names()
            .map_err(|e| KoeError::Config(format!("Kitten ONNX list voices.npz: {e}")))?;
        let mut embeddings = HashMap::with_capacity(names.len());

        for name in names {
            let key = name.trim_end_matches(".npy").to_string();
            let array: Array2<f32> = npz
                .by_name(&name)
                .map_err(|e| KoeError::Config(format!("Kitten ONNX load voice {name}: {e}")))?;
            embeddings.insert(key, array);
        }

        Ok(embeddings)
    }

    fn resolve_voice_selection(
        config: &TtsConfig,
        metadata: &KittenModelMetadata,
    ) -> Result<(String, i32)> {
        let preset = config.preset_voice.trim();
        if !preset.is_empty() {
            if let Some(voice_key) = metadata.voice_aliases.get(preset) {
                return Ok((
                    voice_key.clone(),
                    speaker_id_for_preset_voice(preset).unwrap_or(config.speaker_id),
                ));
            }
            if metadata.speed_priors.contains_key(preset) {
                return Ok((
                    preset.to_string(),
                    speaker_id_for_preset_voice(preset).unwrap_or(config.speaker_id),
                ));
            }
            return Err(KoeError::Config(format!(
                "Kitten ONNX unknown preset_voice: {}",
                preset
            )));
        }

        let fallback_alias = preset_voice_for_speaker_id(config.speaker_id);
        let voice_key = metadata
            .voice_aliases
            .get(fallback_alias)
            .cloned()
            .ok_or_else(|| {
                KoeError::Config(format!(
                    "Kitten ONNX missing voice alias mapping for {}",
                    fallback_alias
                ))
            })?;
        let speaker_id = speaker_id_for_preset_voice(fallback_alias).unwrap_or(1);
        Ok((voice_key, speaker_id))
    }

    fn build_symbol_id_map() -> HashMap<char, i64> {
        let mut map = HashMap::new();
        for (idx, ch) in std::iter::once('$')
            .chain(KITTEN_PUNCTUATION.chars())
            .chain(KITTEN_LETTERS.chars())
            .chain(KITTEN_LETTERS_IPA.chars())
            .enumerate()
        {
            map.insert(ch, idx as i64);
        }
        map
    }

    fn encode_phoneme_text(symbol_ids: &HashMap<char, i64>, phoneme_text: &str) -> Vec<i64> {
        let mut ids = Vec::with_capacity(phoneme_text.len() + 3);
        ids.push(KITTEN_SYMBOL_PADDING_ID);
        let mut previous_was_space = false;
        for ch in phoneme_text.chars() {
            if matches!(ch, '\u{200c}' | '\u{200d}' | '\u{fe0f}') {
                continue;
            }
            let normalized = if ch.is_whitespace() { ' ' } else { ch };
            if normalized == ' ' {
                if previous_was_space {
                    continue;
                }
                previous_was_space = true;
            } else {
                previous_was_space = false;
            }
            if let Some(id) = symbol_ids.get(&normalized) {
                ids.push(*id);
            }
        }
        ids.push(KITTEN_SYMBOL_EOS_ID);
        ids.push(KITTEN_SYMBOL_PADDING_ID);
        ids
    }

    fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
        let mut sentences = Vec::new();
        let mut start = 0usize;
        for (index, ch) in text.char_indices() {
            if is_sentence_boundary(text, index, ch) {
                let end = index + ch.len_utf8();
                sentences.push(text[start..end].to_string());
                start = end;
            }
        }
        if start < text.len() {
            sentences.push(text[start..].to_string());
        }

        let mut chunks = Vec::new();
        for sentence in sentences {
            let sentence = sentence.trim();
            if sentence.is_empty() {
                continue;
            }
            if sentence.chars().count() <= max_len {
                chunks.push(ensure_punctuation(sentence));
                continue;
            }

            let mut temp_chunk = String::new();
            for word in sentence.split_whitespace() {
                let projected_len = temp_chunk.chars().count()
                    + word.chars().count()
                    + usize::from(!temp_chunk.is_empty());
                if projected_len <= max_len {
                    if !temp_chunk.is_empty() {
                        temp_chunk.push(' ');
                    }
                    temp_chunk.push_str(word);
                } else {
                    if !temp_chunk.is_empty() {
                        chunks.push(ensure_punctuation(temp_chunk.trim()));
                    }
                    temp_chunk.clear();
                    temp_chunk.push_str(word);
                }
            }
            if !temp_chunk.is_empty() {
                chunks.push(ensure_punctuation(temp_chunk.trim()));
            }
        }
        chunks
    }

    fn ensure_punctuation(text: &str) -> String {
        let text = text.trim();
        if text.is_empty() {
            return String::new();
        }
        match text.chars().last() {
            Some('.' | '!' | '?' | ',' | ';' | ':') => text.to_string(),
            _ => format!("{text},"),
        }
    }

    fn is_sentence_boundary(text: &str, index: usize, ch: char) -> bool {
        if !matches!(ch, '.' | '!' | '?') {
            return false;
        }
        if ch == '.' {
            let prev = text[..index].chars().last();
            let next = text[index + ch.len_utf8()..].chars().next();
            if prev.is_some_and(|c| c.is_ascii_digit()) && next.is_some_and(|c| c.is_ascii_digit())
            {
                return false;
            }

            let token = text[..index]
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
                .to_ascii_lowercase();
            if matches!(
                token.as_str(),
                "dr" | "prof"
                    | "mr"
                    | "mrs"
                    | "ms"
                    | "fig"
                    | "figs"
                    | "pp"
                    | "p"
                    | "ch"
                    | "sec"
                    | "jan"
                    | "feb"
                    | "mar"
                    | "apr"
                    | "jun"
                    | "jul"
                    | "aug"
                    | "sep"
                    | "sept"
                    | "oct"
                    | "nov"
                    | "dec"
                    | "al"
            ) {
                return false;
            }
            if matches!(token.as_str(), "a" | "p")
                && text[index + ch.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&'m'))
            {
                return false;
            }
        }

        let next_text = text[index + ch.len_utf8()..].trim_start();
        next_text.is_empty() || next_text.chars().next().is_some_and(char::is_uppercase)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;
        use std::fs;

        #[test]
        fn kitten_chunk_text_preserves_common_abbreviations() {
            let chunks = chunk_text("Dr. Smith arrived at 3.14 p.m. sharp. Hello world", 400);
            assert_eq!(
                chunks,
                vec!["Dr. Smith arrived at 3.14 p.m. sharp.", "Hello world,"]
            );
        }

        #[test]
        fn kitten_symbol_map_matches_expected_special_ids() {
            let map = build_symbol_id_map();
            assert_eq!(map.get(&'$').copied(), Some(0));
            assert_eq!(map.get(&'A').copied(), Some(17));
            assert_eq!(map.get(&'"').copied(), Some(15));
        }

        #[test]
        fn kitten_resolves_voice_aliases_from_metadata() {
            let metadata = KittenModelMetadata {
                model_type: "ONNX2".into(),
                model_file: "kitten_tts_nano_v0_8.onnx".into(),
                voices: "voices.npz".into(),
                speed_priors: HashMap::from([("expr-voice-2-m".into(), 0.8)]),
                voice_aliases: HashMap::from([
                    ("Bella".into(), "expr-voice-2-f".into()),
                    ("Jasper".into(), "expr-voice-2-m".into()),
                ]),
            };
            let config = TtsConfig {
                preset_voice: "Bella".into(),
                speaker_id: 99,
                ..TtsConfig::default()
            };
            let (voice_key, speaker_id) = resolve_voice_selection(&config, &metadata).unwrap();
            assert_eq!(voice_key, "expr-voice-2-f");
            assert_eq!(speaker_id, 0);
        }

        #[test]
        fn kitten_model_files_ready_requires_config_voices_and_onnx() {
            let tmp = std::env::temp_dir().join(format!(
                "koe-kitten-ready-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&tmp).unwrap();
            fs::write(tmp.join("config.json"), b"{}").unwrap();
            fs::write(tmp.join("voices.npz"), b"x").unwrap();
            assert!(!model_files_ready(&tmp));
            fs::write(tmp.join("kitten_tts_nano_v0_8.onnx"), b"x").unwrap();
            assert!(model_files_ready(&tmp));
            fs::remove_dir_all(&tmp).unwrap();
        }

        #[test]
        fn kitten_encode_phoneme_text_collapses_extra_spacing() {
            let map = build_symbol_id_map();
            let ids = encode_phoneme_text(&map, "hə  l\u{200d}o");
            let space_id = *map.get(&' ').unwrap();
            assert_eq!(ids[0], KITTEN_SYMBOL_PADDING_ID);
            assert_eq!(ids[ids.len() - 2], KITTEN_SYMBOL_EOS_ID);
            assert_eq!(ids[ids.len() - 1], KITTEN_SYMBOL_PADDING_ID);
            assert_eq!(ids.iter().filter(|&&id| id == space_id).count(), 1);
        }
    }
}

#[cfg(feature = "kitten-onnx")]
pub use imp::KittenOnnxBackend;

#[cfg(not(feature = "kitten-onnx"))]
#[derive(Clone)]
pub struct KittenOnnxBackend;

#[cfg(not(feature = "kitten-onnx"))]
impl KittenOnnxBackend {
    pub fn new(_model_dir: &Path, _config: &TtsConfig) -> Result<Self> {
        Err(KoeError::Config(
            "Kitten ONNX TTS requires the kitten-onnx feature".to_string(),
        ))
    }

    pub fn synthesize(&self, _text: &str) -> Result<(Vec<f32>, u32)> {
        Err(KoeError::Config(
            "Kitten ONNX TTS requires the kitten-onnx feature".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn kitten_language_code_normalizes_english_locales() {
        assert_eq!(kitten_language_code("en"), Some("en"));
        assert_eq!(kitten_language_code("en-US"), Some("en"));
        assert_eq!(kitten_language_code("EN_gb"), Some("en"));
        assert_eq!(kitten_language_code(""), Some("en"));
    }

    #[test]
    fn kitten_language_code_rejects_non_english_locales() {
        assert_eq!(kitten_language_code("zh-CN"), None);
        assert_eq!(kitten_language_code("ja"), None);
        assert_eq!(kitten_language_code("fr"), None);
    }

    #[test]
    fn kitten_speaker_id_accepts_alias_and_voice_id() {
        assert_eq!(speaker_id_for_preset_voice("Bella"), Some(0));
        assert_eq!(speaker_id_for_preset_voice("expr-voice-4-m"), Some(5));
        assert_eq!(speaker_id_for_preset_voice("unknown"), None);
    }

    #[test]
    fn kitten_preset_voice_defaults_to_jasper() {
        assert_eq!(preset_voice_for_speaker_id(1), "Jasper");
        assert_eq!(preset_voice_for_speaker_id(42), "Jasper");
    }

    #[test]
    fn kitten_model_files_ready_requires_config_voices_and_onnx() {
        let tmp = std::env::temp_dir().join(format!(
            "koe-kitten-files-ready-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("config.json"), b"{}").unwrap();
        fs::write(tmp.join("voices.npz"), b"x").unwrap();
        assert!(!model_files_ready(&tmp));
        fs::write(tmp.join("kitten_tts_nano_v0_8.onnx"), b"x").unwrap();
        assert!(model_files_ready(&tmp));
        fs::remove_dir_all(&tmp).unwrap();
    }
}
