//! BLACKVAULT — the ZYRAXON speech-to-text bridge.
//!
//! SAPI (see `speech.rs`) only understands the phrases it was given a grammar
//! for, and it does not exist on every Windows install. This module is the
//! second, always-available recogniser: it posts captured audio to the ZYRAXON
//! STT endpoint and matches the returned words against the command list. The
//! endpoint is the project's own service, so there is no third-party account,
//! no per-seat licence and no model download.
//!
//! Two things are deliberate here:
//!
//!   * The audio is encoded as a WAV **in memory**. Nothing is written to disk;
//!     `store_audio` is forced off everywhere and this module never even looks
//!     at the setting.
//!   * Recognition is a *command* path, not an *identity* path. Words coming
//!     back from the network can never unlock the vault on their own: the
//!     passphrase they are compared against is still hashed locally, and the
//!     configured second factor still stands between the words and the desktop.
//!     `auth.rs` enforces that and this module cannot weaken it.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

use crate::error::{BvError, BvResult};

/// The analysis rate everything is resampled to. The service expects raw
/// 16-bit mono PCM at 16 kHz.
pub const SAMPLE_RATE: u32 = 16_000;

use super::speech::SpeechSink;

/// The default endpoint. Overridable from Settings so a self-hosted copy of the
/// service can be used instead.
pub const DEFAULT_ENDPOINT: &str = "https://zyraxonai.lovable.app/api/public/stt";

/// How long a single recognition request may take before it is abandoned. The
/// live stream gets a longer budget because it stays open while the person
/// speaks; the batch request gets a short one because a command utterance is
/// only a second or two of audio.
const BATCH_TIMEOUT: Duration = Duration::from_secs(12);

/// The transcription of one utterance.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub text: String,
    /// Echoed back by the service, e.g. `bn-BD`. Useful in the status card.
    pub locale: String,
    /// 0.0–1.0. The service returns 0 when it recognised nothing.
    pub confidence: f32,
    /// Which recogniser the service used, e.g. `chromium`.
    pub engine: String,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

// --------------------------------------------------------------------- audio

/// Wrap 16-bit mono PCM in a minimal RIFF/WAVE container.
///
/// The service reads the format from the bytes, so a correct header is all that
/// is needed — no encoder, no ffmpeg, no temporary file.
pub fn wav_from_pcm16(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * (bits / 8) as u32;
    let block_align = channels * (bits / 8);
    let data_len = (pcm.len() * 2) as u32;
    let riff_len = 36 + data_len;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // format = PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Convert f32 samples in −1.0..=1.0 to 16-bit PCM, clipping rather than
/// wrapping so a loud microphone produces distortion instead of noise.
pub fn pcm16_from_f32(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| {
            let v = s.clamp(-1.0, 1.0);
            (v * 32767.0).round() as i16
        })
        .collect()
}

// --------------------------------------------------------------------- parse

/// Parse the batch response body.
///
/// The service returns `{ success, text, language, locale, confidence, engine,
/// container, duration, elapsed }`. A `success: false` body is an error even
/// though the HTTP status was 200, so it is checked explicitly.
pub fn parse_batch_response(body: &str) -> BvResult<Transcript> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| BvError::msg(format!("STT returned invalid JSON: {e}")))?;

    if let Some(false) = v.get("success").and_then(|s| s.as_bool()) {
        let why = v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("the recogniser rejected the audio");
        return Err(BvError::msg(format!("STT failed: {why}")));
    }

    Ok(Transcript {
        text: v.get("text").and_then(|t| t.as_str()).unwrap_or_default().trim().to_string(),
        locale: v
            .get("locale")
            .and_then(|l| l.as_str())
            .or_else(|| v.get("language").and_then(|l| l.as_str()))
            .unwrap_or_default()
            .to_string(),
        confidence: v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0) as f32,
        engine: v.get("engine").and_then(|e| e.as_str()).unwrap_or_default().to_string(),
    })
}

/// Pull the `data:` payload out of one Server-Sent Events block.
///
/// Returns the event name and its JSON payload. A block with no `data:` line
/// (a comment, or the keep-alive the service sends) yields `None`.
pub fn parse_sse_block(block: &str) -> Option<(String, serde_json::Value)> {
    let mut event = String::new();
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        return None;
    }
    let payload = serde_json::from_str(&data).ok()?;
    Some((event, payload))
}

/// The text of a `partial` or `segment` event, if it carries any.
pub fn sse_text(event: &str, payload: &serde_json::Value) -> Option<String> {
    match event {
        "partial" | "segment" | "done" => payload
            .get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
        _ => None,
    }
}

// ------------------------------------------------------------------ requests

/// Build the multipart body for a batch request. Returned whole so the exact
/// bytes can be asserted in a test without a network round trip.
pub fn multipart_body(boundary: &str, wav: &[u8], language: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 512);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    };
    field("language", language);

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"utterance.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// Recognise a finished utterance in one request.
///
/// `endpoint` is the full URL including any query string the user configured;
/// the language is appended to the query when the endpoint does not already
/// carry one, because the service requires it in both modes.
pub fn transcribe(endpoint: &str, language: &str, pcm: &[i16], sample_rate: u32) -> BvResult<Transcript> {
    let wav = wav_from_pcm16(pcm, sample_rate);
    let boundary = "----blackvaultFormBoundary7MA4YWxkTrZu0gW";
    let body = multipart_body(boundary, &wav, language);
    let url = with_language(endpoint, language);

    let agent = agent(BATCH_TIMEOUT);
    let resp = agent
        .post(&url)
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .set("Accept", "application/json")
        .send_bytes(&body)
        .map_err(|e| BvError::msg(format!("STT request failed: {e}")))?;

    let status = resp.status();
    let mut text = String::new();
    resp.into_reader()
        .take(64 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| BvError::msg(format!("STT response could not be read: {e}")))?;

    if !(200..300).contains(&status) {
        return Err(BvError::msg(format!("STT returned HTTP {status}")));
    }
    parse_batch_response(&text)
}

/// Live transcription. Streams `pcm` as raw 16-bit mono little-endian PCM and
/// calls `on_text` for every partial and committed segment that arrives.
///
/// The callback is what makes this usable while the person is still speaking:
/// a partial can already match a command, so "next" lands without waiting for
/// the sentence to finish.
pub fn transcribe_live<F>(
    endpoint: &str,
    language: &str,
    pcm: &[i16],
    sample_rate: u32,
    mut on_text: F,
) -> BvResult<Transcript>
where
    F: FnMut(&str, bool),
{
    let url = with_language(endpoint, language);
    let agent = agent(BATCH_TIMEOUT);
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        bytes.extend_from_slice(&s.to_le_bytes());
    }

    let resp = agent
        .post(&url)
        .set("Content-Type", &format!("audio/l16; rate={sample_rate}"))
        .set("Accept", "text/event-stream")
        .send_bytes(&bytes)
        .map_err(|e| BvError::msg(format!("STT live request failed: {e}")))?;

    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(BvError::msg(format!("STT live returned HTTP {status}")));
    }

    let mut reader = std::io::BufReader::new(resp.into_reader());
    let mut buf = String::new();
    let mut block = String::new();
    let mut best = Transcript::default();

    loop {
        buf.clear();
        let n = std::io::BufRead::read_line(&mut reader, &mut buf)
            .map_err(|e| BvError::msg(format!("STT stream broke: {e}")))?;
        if n == 0 {
            break;
        }
        if buf.trim().is_empty() {
            if let Some((event, payload)) = parse_sse_block(&block) {
                if let Some(text) = sse_text(&event, &payload) {
                    best.text = text.clone();
                    if let Some(c) = payload.get("confidence").and_then(|c| c.as_f64()) {
                        best.confidence = c as f32;
                    }
                    on_text(&text, event == "segment" || event == "done");
                }
                if event == "done" {
                    break;
                }
            }
            block.clear();
        } else {
            block.push_str(&buf);
        }
    }
    Ok(best)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(concat!("BLACKVAULT/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Append `language=` to the endpoint unless it already carries one.
fn with_language(endpoint: &str, language: &str) -> String {
    let endpoint = endpoint.trim();
    let language = language.trim();
    if language.is_empty() || endpoint.contains("language=") {
        return endpoint.to_string();
    }
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{sep}language={language}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let pcm = vec![0i16; 320]; // 20 ms at 16 kHz
        let wav = wav_from_pcm16(&pcm, 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // data length field
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len, 640);
        assert_eq!(wav.len(), 44 + 640);
        // sample rate
        let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
        assert_eq!(rate, 16_000);
        // mono, 16-bit
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
    }

    #[test]
    fn pcm_conversion_clips_instead_of_wrapping() {
        let out = pcm16_from_f32(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5]);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 32767);
        assert_eq!(out[2], -32767);
        assert_eq!(out[3], 32767, "over-range must clip, not wrap");
        assert_eq!(out[4], -32767);
        assert!(out[5] > 16_000 && out[5] < 16_500);
    }

    #[test]
    fn batch_response_parses() {
        let body = r#"{"success":true,"text":" next ","language":"en","locale":"en-US",
                       "confidence":0.82,"engine":"chromium","container":"wav",
                       "duration":1.5,"elapsed":0.2}"#;
        let t = parse_batch_response(body).unwrap();
        assert_eq!(t.text, "next");
        assert_eq!(t.locale, "en-US");
        assert_eq!(t.engine, "chromium");
        assert!((t.confidence - 0.82).abs() < 1e-6);
    }

    #[test]
    fn batch_response_with_empty_text_is_not_an_error() {
        let body = r#"{"success":true,"text":"","language":"en","locale":"en-US","confidence":0}"#;
        let t = parse_batch_response(body).unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn batch_failure_is_reported() {
        let body = r#"{"success":false,"error":"language is required"}"#;
        let err = parse_batch_response(body).unwrap_err().to_string();
        assert!(err.contains("language is required"), "got: {err}");
    }

    #[test]
    fn batch_garbage_is_reported() {
        assert!(parse_batch_response("<html>nope</html>").is_err());
    }

    #[test]
    fn sse_blocks_parse() {
        let block = "event: partial\ndata: {\"text\":\"আজ আকাশ\"}\n";
        let (event, payload) = parse_sse_block(block).unwrap();
        assert_eq!(event, "partial");
        assert_eq!(sse_text(&event, &payload).as_deref(), Some("আজ আকাশ"));
    }

    #[test]
    fn sse_keepalive_yields_nothing() {
        assert!(parse_sse_block(": keep-alive\n").is_none());
        assert!(parse_sse_block("").is_none());
    }

    #[test]
    fn sse_ready_carries_no_text() {
        let block = "event: ready\ndata: {\"rate\":16000,\"language\":\"en\"}\n";
        let (event, payload) = parse_sse_block(block).unwrap();
        assert_eq!(event, "ready");
        assert_eq!(sse_text(&event, &payload), None);
    }

    #[test]
    fn multipart_body_has_both_fields() {
        let body = multipart_body("BOUND", b"RIFFdata", "en");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"language\""));
        assert!(text.contains("\r\n\r\nen\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"utterance.wav\""));
        assert!(text.contains("Content-Type: audio/wav"));
        assert!(text.ends_with("--BOUND--\r\n"));
    }

    #[test]
    fn language_is_appended_only_once() {
        assert_eq!(
            with_language("https://x/api/public/stt", "en"),
            "https://x/api/public/stt?language=en"
        );
        assert_eq!(
            with_language("https://x/api/public/stt?segment=4", "bn"),
            "https://x/api/public/stt?segment=4&language=bn"
        );
        assert_eq!(
            with_language("https://x/api/public/stt?language=bn", "en"),
            "https://x/api/public/stt?language=bn"
        );
    }
}

// -------------------------------------------------------------------- worker

/// Settings the worker needs on every job.
#[derive(Debug, Clone, PartialEq)]
pub struct SttConfig {
    pub endpoint: String,
    /// `en`, `en-US`, `bn` or `bn-BD`.
    pub language: String,
    /// Use the Server-Sent-Events live mode instead of one batch request.
    pub live: bool,
    /// Transcripts below this confidence are ignored. 0 disables the check.
    pub min_confidence: f32,
}

impl Default for SttConfig {
    fn default() -> Self {
        SttConfig {
            endpoint: DEFAULT_ENDPOINT.into(),
            language: "en".into(),
            live: false,
            min_confidence: 0.0,
        }
    }
}

impl SttConfig {
    pub fn sanitized(mut self) -> Self {
        self.endpoint = self.endpoint.trim().to_string();
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            self.endpoint = DEFAULT_ENDPOINT.into();
        }
        let lang = self.language.trim().to_ascii_lowercase();
        self.language = match lang.as_str() {
            "bn" | "bn-bd" | "bengali" => "bn".into(),
            // "both" runs the utterance through the English and the Bengali
            // recogniser and tries whichever comes back. It is the only way to
            // have English commands *and* Bengali speech at the same time,
            // because the service does not detect the language.
            "both" | "en+bn" | "bn+en" | "auto" => "both".into(),
            _ => "en".into(),
        };
        self.min_confidence = self.min_confidence.clamp(0.0, 1.0);
        self
    }
}

/// A background recogniser.
///
/// Recognition is a network call, so it must never run on the audio thread: a
/// slow response would stall capture and drop the next command. Jobs go onto a
/// one-deep queue instead — if the recogniser is still working on the previous
/// utterance the new one is dropped, which is the right behaviour for commands
/// (a stale command must not fire a second later).
pub struct SttWorker {
    tx: Mutex<Option<SyncSender<Vec<i16>>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    busy: Arc<AtomicBool>,
    config: Arc<Mutex<SttConfig>>,
    last_error: Arc<Mutex<Option<String>>>,
    last_text: Arc<Mutex<Option<String>>>,
    seen: Arc<AtomicU64>,
}

impl SttWorker {
    pub fn new() -> Self {
        SttWorker {
            tx: Mutex::new(None),
            thread: Mutex::new(None),
            busy: Arc::new(AtomicBool::new(false)),
            config: Arc::new(Mutex::new(SttConfig::default())),
            last_error: Arc::new(Mutex::new(None)),
            last_text: Arc::new(Mutex::new(None)),
            seen: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_config(&self, config: SttConfig) {
        *self.config.lock() = config.sanitized();
    }

    pub fn config(&self) -> SttConfig {
        self.config.lock().clone()
    }

    pub fn is_running(&self) -> bool {
        self.tx.lock().is_some()
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }

    pub fn last_text(&self) -> Option<String> {
        self.last_text.lock().clone()
    }

    /// Utterances handed to the recogniser since it started.
    pub fn seen(&self) -> u64 {
        self.seen.load(Ordering::Relaxed)
    }

    /// Start the worker. `sink` is called with the recognised text on the
    /// worker thread. Idempotent.
    pub fn start(&self, sink: SpeechSink) {
        if self.is_running() {
            return;
        }
        // Depth 1: one utterance in flight, the next is dropped rather than
        // queued, so a command can never fire seconds after it was spoken.
        let (tx, rx) = sync_channel::<Vec<i16>>(1);
        let busy = Arc::clone(&self.busy);
        let config = Arc::clone(&self.config);
        let last_error = Arc::clone(&self.last_error);
        let last_text = Arc::clone(&self.last_text);
        let seen = Arc::clone(&self.seen);

        let handle = std::thread::Builder::new()
            .name("blackvault-stt".into())
            .spawn(move || {
                while let Ok(pcm) = rx.recv() {
                    busy.store(true, Ordering::Relaxed);
                    seen.fetch_add(1, Ordering::Relaxed);
                    let cfg = config.lock().clone();
                    let result = if cfg.live {
                        // A stream carries one language, so "both" streams the
                        // first of the pair; the batch path is what fans out.
                        transcribe_live(
                            &cfg.endpoint,
                            &cfg.languages()[0],
                            &pcm,
                            SAMPLE_RATE,
                            |text, _final| {
                                *last_text.lock() = Some(text.to_string());
                                sink(text.to_string());
                            },
                        )
                    } else {
                        transcribe_all(&cfg, &pcm).map(|v| v.into_iter().next().unwrap_or_default())
                    };
                    match result {
                        Ok(t) => {
                            *last_error.lock() = None;
                            if !t.is_empty() && t.confidence >= cfg.min_confidence {
                                *last_text.lock() = Some(t.text.clone());
                                // The live path has already delivered every
                                // partial, so only the batch path needs this.
                                if !cfg.live {
                                    sink(t.text);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("speech-to-text failed: {e}");
                            *last_error.lock() = Some(e.to_string());
                        }
                    }
                    busy.store(false, Ordering::Relaxed);
                }
            })
            .ok();

        *self.thread.lock() = handle;
        *self.tx.lock() = Some(tx);
    }

    pub fn stop(&self) {
        if let Some(tx) = self.tx.lock().take() {
            drop(tx);
        }
        if let Some(h) = self.thread.lock().take() {
            let _ = h.join();
        }
        self.busy.store(false, Ordering::Relaxed);
    }

    /// Queue an utterance. Returns false when the worker is busy or stopped, in
    /// which case the utterance is deliberately dropped rather than queued.
    pub fn submit(&self, pcm: Vec<i16>) -> bool {
        if self.is_busy() {
            return false;
        }
        let guard = self.tx.lock();
        let Some(tx) = guard.as_ref() else {
            return false;
        };
        tx.try_send(pcm).is_ok()
    }
}

impl Default for SttWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl SttConfig {
    /// The languages this configuration recognises, in the order to try them.
    pub fn languages(&self) -> Vec<String> {
        match self.language.as_str() {
            "both" => vec!["en".into(), "bn".into()],
            other => vec![other.to_string()],
        }
    }
}

/// Recognise an utterance in every language the configuration asks for.
///
/// The requests run in parallel, so "both" costs about as much as one language
/// rather than twice as much.
pub fn transcribe_all(config: &SttConfig, pcm: &[i16]) -> BvResult<Vec<Transcript>> {
    let cfg = config.clone().sanitized();
    let langs = cfg.languages();
    if langs.len() == 1 {
        return transcribe(&cfg.endpoint, &langs[0], pcm, SAMPLE_RATE).map(|t| vec![t]);
    }

    let mut handles = Vec::new();
    for lang in langs {
        let endpoint = cfg.endpoint.clone();
        let owned = pcm.to_vec();
        handles.push(std::thread::spawn(move || {
            transcribe(&endpoint, &lang, &owned, SAMPLE_RATE)
        }));
    }
    let mut out = Vec::new();
    let mut last_err = None;
    for h in handles {
        match h.join() {
            Ok(Ok(t)) => out.push(t),
            Ok(Err(e)) => last_err = Some(e),
            Err(_) => last_err = Some(BvError::msg("STT worker thread panicked")),
        }
    }
    if out.is_empty() {
        return Err(last_err.unwrap_or_else(|| BvError::msg("no transcript")));
    }
    Ok(out)
}

/// Test a configuration without the audio: a short silent WAV is enough to tell
/// a reachable endpoint from a dead one.
pub fn probe(config: &SttConfig) -> BvResult<Transcript> {
    let cfg = config.clone().sanitized();
    let silence = vec![0i16; SAMPLE_RATE as usize / 2];
    transcribe(&cfg.endpoint, &cfg.languages()[0], &silence, SAMPLE_RATE)
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn config_rejects_a_non_http_endpoint() {
        let c = SttConfig {
            endpoint: "ftp://evil".into(),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(c.endpoint, DEFAULT_ENDPOINT);
    }

    #[test]
    fn config_normalises_the_language() {
        let c = SttConfig { language: "BN-BD".into(), ..Default::default() }.sanitized();
        assert_eq!(c.language, "bn");
        let c = SttConfig { language: "  EN-us ".into(), ..Default::default() }.sanitized();
        assert_eq!(c.language, "en");
        let c = SttConfig { language: "klingon".into(), ..Default::default() }.sanitized();
        assert_eq!(c.language, "en");
    }

    #[test]
    fn config_clamps_confidence() {
        let c = SttConfig { min_confidence: 5.0, ..Default::default() }.sanitized();
        assert_eq!(c.min_confidence, 1.0);
    }

    #[test]
    fn submitting_while_stopped_is_refused() {
        let w = SttWorker::new();
        assert!(!w.submit(vec![0i16; 100]));
        assert!(!w.is_running());
    }

    #[test]
    fn a_busy_worker_drops_rather_than_queues() {
        let w = SttWorker::new();
        w.busy.store(true, Ordering::Relaxed);
        w.start(Arc::new(|_| {}));
        assert!(w.is_running());
        assert!(!w.submit(vec![0i16; 100]), "a busy recogniser must drop the utterance");
        w.stop();
        assert!(!w.is_running());
    }
}
