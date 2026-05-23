use crate::errors::{KoeError, Result};
#[cfg(not(feature = "local-mt"))]
use std::path::Path;
#[cfg(not(feature = "local-mt"))]
use std::sync::Arc;

pub trait LocalMtBackend: Send + Sync {
    fn translate(&self, text: &str, target_lang: &str) -> Result<String>;
    fn model_id(&self) -> &str;
}

#[cfg(feature = "local-mt")]
mod imp {
    use super::*;
    use ndarray::{Array2, ArrayD, IxDyn};
    use ort::{inputs, session::Session, value::Tensor};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tokenizers::Tokenizer;

    const DEFAULT_MAX_LENGTH: usize = 256;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum LocalMtModelFamily {
        OpusMt,
        Nllb200,
    }

    pub fn load_backend(model_path: &Path, source_lang: Option<&str>) -> Result<Arc<dyn LocalMtBackend>> {
        let model_id = model_id_from_path(model_path);
        let backend = MarianBackend::new(&model_id, model_path, source_lang.unwrap_or("auto"))?;
        Ok(Arc::new(backend))
    }

    pub fn provider_requires_source_language(model_path: &Path) -> bool {
        matches!(model_family(&model_id_from_path(model_path)), LocalMtModelFamily::Nllb200)
    }

    pub fn model_files_ready(model_path: &Path) -> bool {
        let has_encoder = model_path.join("encoder_model.onnx").exists();
        let has_decoder = model_path.join("decoder_model_merged.onnx").exists()
            || model_path.join("decoder_model.onnx").exists();
        let has_tokenizer = model_path.join("tokenizer.json").exists();
        has_encoder && has_decoder && has_tokenizer
    }

    fn model_id_from_path(model_path: &Path) -> String {
        model_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    }

    fn model_family(model_id: &str) -> LocalMtModelFamily {
        if model_id.starts_with("nllb-") {
            LocalMtModelFamily::Nllb200
        } else {
            LocalMtModelFamily::OpusMt
        }
    }

    fn iso_to_nllb(lang: &str) -> String {
        match lang {
            "zh" | "zh-CN" | "zho_Hans" => "zho_Hans",
            "zh-TW" | "zho_Hant" => "zho_Hant",
            "en" | "eng_Latn" => "eng_Latn",
            "ja" | "jpn_Jpan" => "jpn_Jpan",
            "ko" | "kor_Hang" => "kor_Hang",
            "fr" | "fra_Latn" => "fra_Latn",
            "de" | "deu_Latn" => "deu_Latn",
            "es" | "spa_Latn" => "spa_Latn",
            "ru" | "rus_Cyrl" => "rus_Cyrl",
            "ar" | "arb_Arab" => "arb_Arab",
            other => other,
        }
        .to_string()
    }

    struct MarianBackend {
        model_id: String,
        family: LocalMtModelFamily,
        source_lang: String,
        encoder: Mutex<Session>,
        decoder: Mutex<Session>,
        tokenizer: Tokenizer,
    }

    impl MarianBackend {
        fn new(model_id: &str, dir: &Path, source_lang: &str) -> Result<Self> {
            let encoder_path = require_file(dir, "encoder_model.onnx")?;
            let decoder_path = optional_file(dir, "decoder_model_merged.onnx")
                .or_else(|| optional_file(dir, "decoder_model.onnx"))
                .ok_or_else(|| {
                    KoeError::Config(format!(
                        "local MT decoder_model_merged.onnx or decoder_model.onnx not found in {}",
                        dir.display()
                    ))
                })?;
            let tokenizer_path = require_file(dir, "tokenizer.json")?;

            let encoder = Session::builder()
                .map_err(|e| KoeError::Config(format!("local MT encoder builder: {e}")))?
                .with_intra_threads(2)
                .map_err(|e| KoeError::Config(format!("local MT encoder threads: {e}")))?
                .commit_from_file(&encoder_path)
                .map_err(|e| KoeError::Config(format!("local MT load encoder: {e}")))?;
            let decoder = Session::builder()
                .map_err(|e| KoeError::Config(format!("local MT decoder builder: {e}")))?
                .with_intra_threads(2)
                .map_err(|e| KoeError::Config(format!("local MT decoder threads: {e}")))?
                .commit_from_file(&decoder_path)
                .map_err(|e| KoeError::Config(format!("local MT load decoder: {e}")))?;
            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| KoeError::Config(format!("local MT tokenizer: {e}")))?;

            log::info!(
                "[translation] local MT backend loaded from {} ({model_id})",
                dir.display()
            );

            Ok(Self {
                model_id: model_id.to_string(),
                family: model_family(model_id),
                source_lang: source_lang.to_string(),
                encoder: Mutex::new(encoder),
                decoder: Mutex::new(decoder),
                tokenizer,
            })
        }

        fn encode_text(&self, text: &str) -> Result<Vec<i64>> {
            let encoding = self
                .tokenizer
                .encode(text, true)
                .map_err(|e| KoeError::Config(format!("local MT encode: {e}")))?;
            let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();

            if matches!(self.family, LocalMtModelFamily::Nllb200) {
                let src_tag = iso_to_nllb(&self.source_lang);
                if let Some(&tok_id) = self.tokenizer.get_vocab(true).get(src_tag.as_str()) {
                    ids.insert(0, tok_id as i64);
                }
            }

            let eos_id = self.eos_token_id();
            if ids.last() != Some(&eos_id) {
                ids.push(eos_id);
            }
            Ok(ids)
        }

        fn run_encoder(&self, input_ids: &[i64]) -> Result<ArrayD<f32>> {
            let seq_len = input_ids.len();
            let ids_arr = Array2::from_shape_vec((1, seq_len), input_ids.to_vec())
                .map_err(|e| KoeError::Config(format!("local MT encoder shape: {e}")))?;
            let mask_arr = Array2::<i64>::ones((1, seq_len));

            let ids_tensor = Tensor::from_array(ids_arr)
                .map_err(|e| KoeError::Config(format!("local MT ids tensor: {e}")))?;
            let mask_tensor = Tensor::from_array(mask_arr)
                .map_err(|e| KoeError::Config(format!("local MT mask tensor: {e}")))?;

            let mut encoder = self.encoder.lock().unwrap();
            let outputs = encoder
                .run(inputs!["input_ids" => ids_tensor, "attention_mask" => mask_tensor])
                .map_err(|e| KoeError::Config(format!("local MT encoder run: {e}")))?;

            let (shape, data) = outputs["last_hidden_state"]
                .try_extract_tensor::<f32>()
                .map_err(|e| KoeError::Config(format!("local MT hidden state: {e}")))?;
            let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
            ArrayD::from_shape_vec(IxDyn(&dims), data.to_vec())
                .map_err(|e| KoeError::Config(format!("local MT hidden reshape: {e}")))
        }

        fn greedy_decode(&self, encoder_hidden: &ArrayD<f32>, target_lang: &str) -> Result<Vec<i64>> {
            let bos_id = self
                .target_lang_token_id(target_lang)
                .unwrap_or_else(|| self.bos_token_id());
            let eos_id = self.eos_token_id();
            let enc_seq_len = encoder_hidden.shape()[1];
            let enc_shape = encoder_hidden.shape().to_vec();
            let mut generated = vec![bos_id];

            for _ in 0..DEFAULT_MAX_LENGTH {
                let dec_len = generated.len();
                let dec_ids = Array2::from_shape_vec((1, dec_len), generated.clone())
                    .map_err(|e| KoeError::Config(format!("local MT decoder shape: {e}")))?;
                let enc_mask = Array2::<i64>::ones((1, enc_seq_len));
                let enc_hidden = ArrayD::from_shape_vec(
                    IxDyn(&enc_shape),
                    encoder_hidden.iter().copied().collect(),
                )
                .map_err(|e| KoeError::Config(format!("local MT hidden clone: {e}")))?;

                let dec_ids_t = Tensor::from_array(dec_ids)
                    .map_err(|e| KoeError::Config(format!("local MT decoder ids tensor: {e}")))?;
                let enc_mask_t = Tensor::from_array(enc_mask)
                    .map_err(|e| KoeError::Config(format!("local MT decoder mask tensor: {e}")))?;
                let enc_hidden_t = Tensor::from_array(enc_hidden)
                    .map_err(|e| KoeError::Config(format!("local MT decoder hidden tensor: {e}")))?;

                let mut decoder = self.decoder.lock().unwrap();
                let outputs = decoder
                    .run(inputs![
                        "input_ids" => dec_ids_t,
                        "encoder_attention_mask" => enc_mask_t,
                        "encoder_hidden_states" => enc_hidden_t
                    ])
                    .map_err(|e| KoeError::Config(format!("local MT decoder run: {e}")))?;

                let (shape, logits) = outputs["logits"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| KoeError::Config(format!("local MT logits: {e}")))?;
                let vocab_size = shape[2] as usize;
                let last_step_start = logits.len().saturating_sub(vocab_size);
                let next_id = logits[last_step_start..]
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx as i64)
                    .unwrap_or(eos_id);

                if next_id == eos_id {
                    break;
                }
                generated.push(next_id);
            }

            if generated.first() == Some(&bos_id) {
                generated.remove(0);
            }
            Ok(generated)
        }

        fn bos_token_id(&self) -> i64 {
            if let Some(tok) = self.tokenizer.get_vocab(true).get("<pad>") {
                return *tok as i64;
            }
            if let Some(tok) = self.tokenizer.get_vocab(true).get("[BOS]") {
                return *tok as i64;
            }
            0
        }

        fn eos_token_id(&self) -> i64 {
            if let Some(tok) = self.tokenizer.get_vocab(true).get("</s>") {
                return *tok as i64;
            }
            if let Some(tok) = self.tokenizer.get_vocab(true).get("[EOS]") {
                return *tok as i64;
            }
            2
        }

        fn target_lang_token_id(&self, target_lang: &str) -> Option<i64> {
            match self.family {
                LocalMtModelFamily::Nllb200 => {
                    let tag = iso_to_nllb(target_lang);
                    self.tokenizer
                        .get_vocab(true)
                        .get(tag.as_str())
                        .map(|&id| id as i64)
                }
                LocalMtModelFamily::OpusMt => None,
            }
        }

        fn decode_ids(&self, ids: &[i64]) -> Result<String> {
            let ids: Vec<u32> = ids.iter().map(|&id| id as u32).collect();
            self.tokenizer
                .decode(&ids, true)
                .map_err(|e| KoeError::Config(format!("local MT decode: {e}")))
        }
    }

    impl LocalMtBackend for MarianBackend {
        fn translate(&self, text: &str, target_lang: &str) -> Result<String> {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(String::new());
            }
            let input_ids = self.encode_text(trimmed)?;
            let encoder_hidden = self.run_encoder(&input_ids)?;
            let output_ids = self.greedy_decode(&encoder_hidden, target_lang)?;
            let translated = self.decode_ids(&output_ids)?;
            Ok(translated.trim().to_string())
        }

        fn model_id(&self) -> &str {
            &self.model_id
        }
    }

    fn require_file(dir: &Path, name: &str) -> Result<PathBuf> {
        let path = dir.join(name);
        if path.exists() {
            Ok(path)
        } else {
            Err(KoeError::Config(format!(
                "local MT file not found: {}",
                path.display()
            )))
        }
    }

    fn optional_file(dir: &Path, name: &str) -> Option<PathBuf> {
        let path = dir.join(name);
        path.exists().then_some(path)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn nllb_models_require_explicit_source_language() {
            assert!(provider_requires_source_language(Path::new("mt-local/nllb-200-distilled-600M")));
            assert!(!provider_requires_source_language(Path::new("mt-local/opus-mt-zh-en")));
        }

        #[test]
        fn iso_codes_map_to_nllb_tags() {
            assert_eq!(iso_to_nllb("zh"), "zho_Hans");
            assert_eq!(iso_to_nllb("en"), "eng_Latn");
            assert_eq!(iso_to_nllb("ja"), "jpn_Jpan");
        }
    }
}

#[cfg(feature = "local-mt")]
pub use imp::{load_backend, model_files_ready, provider_requires_source_language};

#[cfg(not(feature = "local-mt"))]
pub fn load_backend(_model_path: &Path, _source_lang: Option<&str>) -> Result<Arc<dyn LocalMtBackend>> {
    Err(KoeError::Config(
        "local MT support is not compiled into this build".to_string(),
    ))
}

#[cfg(not(feature = "local-mt"))]
pub fn provider_requires_source_language(_model_path: &Path) -> bool {
    false
}

#[cfg(not(feature = "local-mt"))]
pub fn model_files_ready(_model_path: &Path) -> bool {
    false
}
