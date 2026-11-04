//! BLACKVAULT — the always-on voice engine.
//!
//! Architecture:
//!   capture thread (cpal)  →  block channel  →  engine thread
//!                                                  │
//!                     resample to 16 kHz mono ────┤
//!                     energy VAD ─────────────────┤
//!                     MFCC + speaker vector ──────┤
//!                     DTW match vs templates ─────┤
//!                                                 ▼
//!                                     Utterance sink (the core routes it)
//!
//! The engine owns *no* security decisions. It produces utterances and matches;
//! `lib.rs` applies the router and the auth policy. If the audio device
//! disappears or the stream errors, the engine tears the stream down and
//! re-opens it with exponential backoff — it never silently gives up.

pub mod matcher;
pub mod router;
pub mod vad;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

use crate::dsp;
use crate::models::VoiceStatus;
use matcher::Template;
use vad::{Vad, BLOCK_MS};

/// A finished utterance, ready to be routed.
#[derive(Debug, Clone)]
pub struct Utterance {
    pub mfcc: Vec<Vec<f32>>,
    pub speaker: Vec<f32>,
    pub duration_ms: u32,
    pub peak_level: f32,
    /// The same audio as 16-bit PCM at 16 kHz, for recognisers that want the
    /// signal rather than the features. Held in memory and dropped with the
    /// utterance: it is never written anywhere.
    pub pcm16: Vec<i16>,
}

pub type UtteranceSink = Arc<dyn Fn(Utterance) + Send + Sync>;
pub type StatusSink = Arc<dyn Fn(VoiceStatus) + Send + Sync>;

struct Shared {
    running: AtomicBool,
    stop: AtomicBool,
    level: AtomicU32,
    recovery: AtomicU32,
    templates: Mutex<Vec<Template>>,
    status: Mutex<VoiceStatus>,
    sink: Mutex<Option<UtteranceSink>>,
    status_sink: Mutex<Option<StatusSink>>,
}

pub struct VoiceEngine {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl VoiceEngine {
    pub fn new() -> Self {
        VoiceEngine {
            shared: Arc::new(Shared {
                running: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                level: AtomicU32::new(0),
                recovery: AtomicU32::new(0),
                templates: Mutex::new(Vec::new()),
                status: Mutex::new(VoiceStatus::default()),
                sink: Mutex::new(None),
                status_sink: Mutex::new(None),
            }),
            thread: Mutex::new(None),
        }
    }

    pub fn set_sink(&self, sink: UtteranceSink) {
        *self.shared.sink.lock() = Some(sink);
    }

    pub fn set_status_sink(&self, sink: StatusSink) {
        *self.shared.status_sink.lock() = Some(sink);
    }

    /// Replace the recognition templates (wake phrase + every enrolled command).
    pub fn set_templates(&self, templates: Vec<Template>) {
        *self.shared.templates.lock() = templates;
    }

    pub fn status(&self) -> VoiceStatus {
        let mut s = self.shared.status.lock().clone();
        s.running = self.shared.running.load(Ordering::Relaxed);
        s.listening = s.running;
        s.recovery_attempts = self.shared.recovery.load(Ordering::Relaxed);
        s
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    fn publish(&self) {
        let s = self.status();
        if let Some(sink) = self.shared.status_sink.lock().as_ref() {
            sink(s);
        }
    }

    /// Start (or restart) capture. Idempotent.
    pub fn start(&self) -> VoiceStatus {
        if self.is_running() {
            return self.status();
        }
        self.shared.stop.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        let handle = std::thread::Builder::new()
            .name("blackvault-voice".into())
            .spawn(move || engine_loop(shared))
            .ok();
        *self.thread.lock() = handle;
        self.shared.running.store(true, Ordering::Relaxed);
        {
            let mut st = self.shared.status.lock();
            st.backend = "dtw".into();
            st.model = Some("built-in MFCC/DTW".into());
        }
        self.publish();
        self.status()
    }

    /// Stop capture and join the thread.
    pub fn stop(&self) -> VoiceStatus {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.thread.lock().take() {
            let _ = h.join();
        }
        self.shared.level.store(0, Ordering::Relaxed);
        self.publish();
        self.status()
    }

    /// Full recovery: stop, reset the failure counter, start again.
    pub fn restart(&self) -> VoiceStatus {
        let _ = self.stop();
        std::thread::sleep(Duration::from_millis(150));
        self.shared.recovery.store(0, Ordering::Relaxed);
        self.start()
    }

    pub fn set_wake_phrase(&self, phrase: Option<String>) {
        self.shared.status.lock().wake_phrase = phrase;
        self.publish();
    }

    pub fn note_heard(&self, text: &str) {
        self.shared.status.lock().last_heard = Some(text.to_string());
        self.publish();
    }

    pub fn note_command(&self, phrase: &str) {
        self.shared.status.lock().last_command = Some(phrase.to_string());
        self.publish();
    }

    pub fn bump_recovery(&self) {
        self.shared.recovery.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// The engine thread body: open a stream, process blocks, recover on failure.
fn engine_loop(shared: Arc<Shared>) {
    let mut backoff = Duration::from_millis(400);
    while !shared.stop.load(Ordering::Relaxed) {
        match run_capture(&shared) {
            Ok(()) => break, // clean stop
            Err(err) => {
                shared.recovery.fetch_add(1, Ordering::Relaxed);
                log::warn!("voice capture failed: {err}; retrying in {:?}", backoff);
                {
                    let mut st = shared.status.lock();
                    st.backend = "dtw".into();
                    st.model = Some(format!("recovering: {err}"));
                }
                if let Some(sink) = shared.status_sink.lock().as_ref() {
                    let mut s = shared.status.lock().clone();
                    s.running = false;
                    sink(s);
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(10));
            }
        }
    }
    shared.running.store(false, Ordering::Relaxed);
}

fn run_capture(shared: &Arc<Shared>) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no input device".to_string())?;
    let device_name = device.name().unwrap_or_else(|_| "Unknown microphone".into());
    let config = device
        .default_input_config()
        .map_err(|e| format!("no input config: {e}"))?;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();
    let channels = stream_config.channels as usize;
    let sample_rate = stream_config.sample_rate.0;

    {
        let mut st = shared.status.lock();
        st.backend = "dtw".into();
        st.model = Some(format!("{device_name} @ {sample_rate} Hz"));
        st.listening = true;
    }

    let (tx, rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = channel();
    let level_cell = Arc::clone(&shared);

    let err_fn = |e| log::error!("input stream error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                let mono = to_mono_f32(data, channels);
                record_level(&level_cell, &mono);
                let _ = tx.send(mono);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let f: Vec<f32> = data.iter().map(|s| *s as f32 / 32768.0).collect();
                let mono = to_mono_f32(&f, channels);
                record_level(&level_cell, &mono);
                let _ = tx.send(mono);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                let f: Vec<f32> = data.iter().map(|s| (*s as f32 / 32768.0) - 1.0).collect();
                let mono = to_mono_f32(&f, channels);
                record_level(&level_cell, &mono);
                let _ = tx.send(mono);
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| format!("could not open stream: {e}"))?;

    stream.play().map_err(|e| format!("could not start stream: {e}"))?;

    // ---- process loop -----------------------------------------------------
    let block_samples = (dsp::SAMPLE_RATE * BLOCK_MS / 1000) as usize;
    let mut resampler = LinearResampler::new(sample_rate, dsp::SAMPLE_RATE);
    let mut pending: Vec<f32> = Vec::new();
    let mut utterance: Vec<f32> = Vec::new();
    let mut vad = Vad::new();
    let mut peak: f32 = 0.0;
    let mut last_activity = Instant::now();

    while !shared.stop.load(Ordering::Relaxed) {
        let block = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(b) => b,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Nothing arriving: if we were mid-utterance, close it out.
                if vad.in_speech() && last_activity.elapsed() > Duration::from_millis(900) {
                    emit(&shared, &mut utterance, &mut peak);
                    vad.reset();
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("capture channel closed".into());
            }
        };

        pending.extend_from_slice(&resampler.push(&block));
        while pending.len() >= block_samples {
            let frame: Vec<f32> = pending.drain(0..block_samples).collect();
            let speaking = vad.push(&frame);
            if speaking {
                utterance.extend_from_slice(&frame);
                let e = dsp::frame_energy(&frame);
                if e > peak {
                    peak = e;
                }
                last_activity = Instant::now();
                // Cap an utterance at 6 seconds so a stuck VAD cannot grow it
                // without bound.
                if utterance.len() > dsp::SAMPLE_RATE as usize * 6 {
                    emit(&shared, &mut utterance, &mut peak);
                }
            } else if !utterance.is_empty() {
                emit(&shared, &mut utterance, &mut peak);
            }
        }
    }

    drop(stream);
    Ok(())
}

fn emit(shared: &Arc<Shared>, utterance: &mut Vec<f32>, peak: &mut f32) {
    if utterance.len() < dsp::SAMPLE_RATE as usize / 8 {
        // Under ~125 ms: a cough, a click. Discard.
        utterance.clear();
        *peak = 0.0;
        return;
    }
    let samples = std::mem::take(utterance);
    let mfcc = dsp::voiced_frames(&samples);
    let speaker = dsp::speaker_vector(&samples);
    let duration_ms = (samples.len() as u64 * 1000 / dsp::SAMPLE_RATE as u64) as u32;
    let peak_level = *peak;
    *peak = 0.0;
    let pcm16 = crate::platform::stt::pcm16_from_f32(&samples);

    let sink = shared.sink.lock().clone();
    if let Some(sink) = sink {
        sink(Utterance {
            mfcc,
            speaker,
            duration_ms,
            peak_level,
            pcm16,
        });
    }
}

fn to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn record_level(shared: &Arc<Shared>, mono: &[f32]) {
    let rms = dsp::frame_energy(mono);
    let scaled = (rms * 6.0).clamp(0.0, 1.0);
    shared.level.store(scaled.to_bits(), Ordering::Relaxed);
}

/// Linear resampler from the device rate to the 16 kHz analysis rate. Linear
/// interpolation is adequate for MFCC extraction and avoids a heavy dependency.
struct LinearResampler {
    ratio: f64,
    pos: f64,
    tail: Vec<f32>,
}

impl LinearResampler {
    fn new(from: u32, to: u32) -> Self {
        LinearResampler {
            ratio: from as f64 / to as f64,
            pos: 0.0,
            tail: Vec::new(),
        }
    }

    fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(input);
        let mut out = Vec::with_capacity((buf.len() as f64 / self.ratio) as usize + 2);
        while (self.pos as usize) + 1 < buf.len() {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = buf[i];
            let b = buf[i + 1];
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        let consumed = self.pos as usize;
        self.tail = buf[consumed..].to_vec();
        self.pos -= consumed as f64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix_averages_channels() {
        let stereo = vec![1.0f32, 0.0, 1.0, 0.0];
        assert_eq!(to_mono_f32(&stereo, 2), vec![0.5, 0.5]);
    }

    #[test]
    fn resampler_halves_length_when_downsampling_by_two() {
        let mut r = LinearResampler::new(32_000, 16_000);
        let input: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let out = r.push(&input);
        assert!(out.len() > 450 && out.len() < 520, "got {}", out.len());
    }

    #[test]
    fn resampler_is_identity_at_target_rate() {
        let mut r = LinearResampler::new(16_000, 16_000);
        let input = vec![0.1f32; 32];
        assert_eq!(r.push(&input), input);
    }
}
