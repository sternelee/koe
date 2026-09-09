// Opt-in debug harness: stream a real speech WAV through DoubaoImeProvider and
// dump every server message and emitted event. Skipped unless DOUBAOIME_DEBUG_WAV
// points at a 16kHz mono s16le WAV; DOUBAOIME_DEBUG_CREDS overrides the cached
// device-credential path. Run with --nocapture. Useful when the IME backend
// changes its result_json shape (it has, twice — most recently growing a second
// per-segment track in `results[1..]` that duplicated every utterance when
// concatenated).
//
//   say -v Tingting "你好 [[slnc 1500]] 今天天气怎么样" -o t.aiff
//   afconvert -f WAVE -d LEI16@16000 -c 1 t.aiff t.wav
//   DOUBAOIME_DEBUG_WAV=$PWD/t.wav cargo test -p koe-asr \
//       --test doubaoime_debug_test -- --nocapture
use koe_asr::{AsrConfig, AsrEvent, AsrProvider};

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        eprintln!("[{}] {}", record.level(), record.args());
    }
    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

#[tokio::test]
async fn doubaoime_debug_stream_wav() {
    let wav_path = match std::env::var("DOUBAOIME_DEBUG_WAV") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("DOUBAOIME_DEBUG_WAV not set; skipping");
            return;
        }
    };

    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Debug);

    let wav = std::fs::read(&wav_path).expect("read wav");
    let pcm = &wav[44..]; // skip canonical WAV header

    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "credential_path".to_string(),
        std::env::var("DOUBAOIME_DEBUG_CREDS")
            .unwrap_or_else(|_| "/tmp/test_doubaoime_creds.json".to_string()),
    );

    let config = AsrConfig {
        connect_timeout_ms: 10000,
        final_wait_timeout_ms: 10000,
        enable_punc: true,
        custom_headers: headers,
        ..Default::default()
    };

    let mut provider = koe_asr::DoubaoImeProvider::new();
    provider.connect(&config).await.expect("connect");
    eprintln!("=== connected, streaming {} bytes of PCM", pcm.len());

    // Stream in 100ms chunks with real-time pacing so server VAD behaves
    // like live dictation.
    const CHUNK: usize = 3200; // 100ms @ 16kHz s16le
    for chunk in pcm.chunks(CHUNK) {
        provider.send_audio(chunk).await.expect("send_audio");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Trailing silence so VAD closes the utterance.
    for _ in 0..8 {
        provider.send_audio(&[0u8; CHUNK]).await.expect("send silence");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    provider.finish_input().await.expect("finish_input");
    eprintln!("=== finished input, reading events");

    loop {
        let ev = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            provider.next_event(),
        )
        .await
        {
            Ok(ev) => ev,
            Err(_) => {
                eprintln!(">>> TIMEOUT waiting for next event");
                break;
            }
        };
        match ev {
            Ok(AsrEvent::Interim(t)) => eprintln!(">>> INTERIM: {t}"),
            Ok(AsrEvent::Definite(t)) => eprintln!(">>> DEFINITE: {t}"),
            Ok(AsrEvent::Final(t)) => eprintln!(">>> FINAL: {t}"),
            Ok(AsrEvent::Closed(r)) => {
                eprintln!(">>> CLOSED: {r:?}");
                break;
            }
            Ok(AsrEvent::Error(e)) => {
                eprintln!(">>> ERROR: {e}");
                break;
            }
            Ok(other) => eprintln!(">>> {other:?}"),
            Err(e) => {
                eprintln!(">>> ERR: {e}");
                break;
            }
        }
    }
    provider.close().await.ok();
}
