use crate::config::config_dir;
use crate::errors::{KoeError, Result};
use std::path::PathBuf;

const SENSEVOICE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
const SILERO_VAD_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";
const WHISPER_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-tiny.en.tar.bz2";

pub fn models_dir() -> PathBuf {
    config_dir().join("models")
}

pub fn sensevoice_dir() -> PathBuf {
    models_dir().join("sensevoice")
}

pub fn whisper_dir() -> PathBuf {
    models_dir().join("whisper")
}

pub fn silero_vad_path() -> PathBuf {
    models_dir().join("silero_vad.onnx")
}

pub fn ensure_sensevoice_models() -> Result<PathBuf> {
    let dir = sensevoice_dir();
    let model_path = dir.join("model.int8.onnx");
    let tokens_path = dir.join("tokens.txt");

    if model_path.exists() && tokens_path.exists() {
        log::debug!("SenseVoice model already exists at {}", dir.display());
        return Ok(dir);
    }

    log::info!("Downloading SenseVoice model to {}", dir.display());
    download_and_extract(SENSEVOICE_URL, &dir)?;

    if !model_path.exists() {
        return Err(KoeError::Config(format!(
            "SenseVoice model not found after download: {}",
            model_path.display()
        )));
    }

    log::info!("SenseVoice model ready");
    Ok(dir)
}

pub fn ensure_whisper_models() -> Result<PathBuf> {
    let dir = whisper_dir();
    let encoder_path = dir.join("tiny.en-encoder.int8.onnx");
    let decoder_path = dir.join("tiny.en-decoder.int8.onnx");
    let tokens_path = dir.join("tiny.en-tokens.txt");

    if encoder_path.exists() && decoder_path.exists() && tokens_path.exists() {
        log::debug!("Whisper model already exists at {}", dir.display());
        return Ok(dir);
    }

    log::info!("Downloading Whisper model to {}", dir.display());
    download_and_extract(WHISPER_URL, &dir)?;

    if !encoder_path.exists() {
        return Err(KoeError::Config(format!(
            "Whisper encoder not found after download: {}",
            encoder_path.display()
        )));
    }

    log::info!("Whisper model ready");
    Ok(dir)
}

pub fn ensure_silero_vad() -> Result<PathBuf> {
    let path = silero_vad_path();

    if path.exists() {
        log::debug!("Silero VAD already exists at {}", path.display());
        return Ok(path);
    }

    log::info!("Downloading Silero VAD model to {}", path.display());
    std::fs::create_dir_all(models_dir())
        .map_err(|e| KoeError::Config(format!("create models dir: {e}")))?;

    download_file(SILERO_VAD_URL, &path)?;

    log::info!("Silero VAD model ready");
    Ok(path)
}

fn download_and_extract(url: &str, dest_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| KoeError::Config(format!("create dest dir: {e}")))?;

    let temp_tar = std::env::temp_dir().join(format!("koe-download-{}", uuid::Uuid::new_v4()));

    download_file(url, &temp_tar)?;

    log::info!("Extracting archive to {}", dest_dir.display());

    let output = std::process::Command::new("tar")
        .args(["-xjf", &temp_tar.to_string_lossy()])
        .current_dir(dest_dir)
        .output()
        .map_err(|e| KoeError::Config(format!("extract tar: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(KoeError::Config(format!("tar extraction failed: {stderr}")));
    }

    let _ = std::fs::remove_file(&temp_tar);

    Ok(())
}

fn download_file(url: &str, dest: &PathBuf) -> Result<()> {
    log::info!("Downloading {} to {}", url, dest.display());

    let mut response = reqwest::blocking::get(url)
        .map_err(|e| KoeError::Config(format!("download {url}: {e}")))?;

    if !response.status().is_success() {
        return Err(KoeError::Config(format!(
            "download failed with status {}",
            response.status()
        )));
    }

    let mut file =
        std::fs::File::create(dest).map_err(|e| KoeError::Config(format!("create file: {e}")))?;

    response
        .copy_to(&mut file)
        .map_err(|e| KoeError::Config(format!("write file: {e}")))?;

    Ok(())
}

pub fn ensure_all_models() -> Result<()> {
    std::fs::create_dir_all(models_dir())
        .map_err(|e| KoeError::Config(format!("create models dir: {e}")))?;

    let _ = ensure_silero_vad()?;
    let _ = ensure_sensevoice_models()?;
    let _ = ensure_whisper_models()?;

    Ok(())
}

pub fn ensure_models_for_provider(provider: &str) -> Result<PathBuf> {
    match provider {
        "sensevoice" => ensure_sensevoice_models(),
        "whisper" => ensure_whisper_models(),
        _ => Ok(config_dir()),
    }
}
