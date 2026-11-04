//! BLACKVAULT — audio feature extraction and matching.
//!
//! This is the offline recognition core. There is no cloud call and no model
//! download required:
//!
//!   * `mfcc()` turns a PCM frame sequence into a sequence of MFCC vectors
//!     (pre-emphasis → Hamming → FFT → mel filterbank → log → DCT-II).
//!   * `dtw()` aligns two MFCC sequences and returns a length-normalised
//!     distance. A user records each command phrase once; later utterances are
//!     matched against those templates.
//!   * `speaker_vector()` reduces an utterance to a fixed 26-dimension vector
//!     (per-coefficient mean and standard deviation) used as a lightweight
//!     speaker check.
//!
//! Everything here is pure and unit-tested, so it can be validated without a
//! microphone.

use num_complex::Complex;
use rustfft::FftPlanner;

pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_LEN: usize = 400; // 25 ms
pub const HOP_LEN: usize = 160; // 10 ms
pub const FFT_LEN: usize = 512;
pub const MEL_FILTERS: usize = 26;
pub const NUM_CEPS: usize = 13;

/// Pre-emphasis coefficient — lifts the high frequencies that carry
/// consonant information.
const PREEMPH: f32 = 0.97;

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10f32.powf(mel / 2595.0) - 1.0)
}

/// Triangular mel filterbank over the positive-frequency bins.
fn mel_filterbank() -> Vec<Vec<f32>> {
    let n_bins = FFT_LEN / 2 + 1;
    let f_min = 80.0f32;
    let f_max = 7600.0f32;
    let mel_min = hz_to_mel(f_min);
    let mel_max = hz_to_mel(f_max);
    let mut points = Vec::with_capacity(MEL_FILTERS + 2);
    for i in 0..(MEL_FILTERS + 2) {
        let mel = mel_min + (mel_max - mel_min) * i as f32 / (MEL_FILTERS + 1) as f32;
        let hz = mel_to_hz(mel);
        // Bin index for this frequency.
        let bin = (hz / SAMPLE_RATE as f32 * FFT_LEN as f32).floor() as usize;
        points.push(bin.min(n_bins - 1));
    }
    let mut filters = vec![vec![0.0f32; n_bins]; MEL_FILTERS];
    for m in 0..MEL_FILTERS {
        let (left, center, right) = (points[m], points[m + 1], points[m + 2]);
        for (bin, weight) in filters[m].iter_mut().enumerate() {
            if bin >= left && bin <= center && center > left {
                *weight = (bin - left) as f32 / (center - left) as f32;
            } else if bin > center && bin <= right && right > center {
                *weight = (right - bin) as f32 / (right - center) as f32;
            }
        }
    }
    filters
}

fn hamming() -> Vec<f32> {
    (0..FRAME_LEN)
        .map(|n| {
            0.54 - 0.46
                * ((2.0 * std::f32::consts::PI * n as f32) / (FRAME_LEN as f32 - 1.0)).cos()
        })
        .collect()
}

/// DCT-II matrix (NUM_CEPS x MEL_FILTERS).
fn dct_matrix() -> Vec<Vec<f32>> {
    let mut m = vec![vec![0.0f32; MEL_FILTERS]; NUM_CEPS];
    for (k, row) in m.iter_mut().enumerate() {
        for (n, cell) in row.iter_mut().enumerate() {
            *cell = ((n as f32 + 0.5) * k as f32 * std::f32::consts::PI / MEL_FILTERS as f32).cos();
        }
        let scale = if k == 0 {
            (1.0 / MEL_FILTERS as f32).sqrt()
        } else {
            (2.0 / MEL_FILTERS as f32).sqrt()
        };
        for cell in row.iter_mut() {
            *cell *= scale;
        }
    }
    m
}

/// Root-mean-square energy of a frame — used by the VAD and the level meter.
pub fn frame_energy(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame.iter().map(|s| s * s).sum();
    (sum / frame.len() as f32).sqrt()
}

/// Extract MFCCs with per-utterance cepstral mean normalisation.
pub fn mfcc(samples: &[f32]) -> Vec<Vec<f32>> {
    if samples.len() < FRAME_LEN {
        return Vec::new();
    }
    let window = hamming();
    let filters = mel_filterbank();
    let dct = dct_matrix();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_LEN);

    let mut frames: Vec<Vec<f32>> = Vec::new();
    let mut pos = 0;
    while pos + FRAME_LEN <= samples.len() {
        let raw = &samples[pos..pos + FRAME_LEN];
        // Pre-emphasis + window, then zero-pad into the FFT buffer.
        let mut buf: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); FFT_LEN];
        let mut prev = samples[pos.saturating_sub(1)];
        for i in 0..FRAME_LEN {
            let emphasised = raw[i] - PREEMPH * prev;
            prev = raw[i];
            buf[i] = Complex::new(emphasised * window[i], 0.0);
        }
        fft.process(&mut buf);
        // Power spectrum (positive bins only).
        let n_bins = FFT_LEN / 2 + 1;
        let power: Vec<f32> = buf[..n_bins].iter().map(|c| c.norm_sqr()).collect();

        // Mel energies → log.
        let mut log_mel = vec![0.0f32; MEL_FILTERS];
        for m in 0..MEL_FILTERS {
            let mut acc = 0.0f32;
            for b in 0..n_bins {
                acc += filters[m][b] * power[b];
            }
            log_mel[m] = (acc.max(1e-10)).ln();
        }
        // DCT → cepstral coefficients.
        let mut ceps = vec![0.0f32; NUM_CEPS];
        for k in 0..NUM_CEPS {
            let mut acc = 0.0f32;
            for n in 0..MEL_FILTERS {
                acc += dct[k][n] * log_mel[n];
            }
            ceps[k] = acc;
        }
        frames.push(ceps);
        pos += HOP_LEN;
    }

    // Cepstral mean normalisation removes channel and loudness bias.
    if !frames.is_empty() {
        let mut mean = vec![0.0f32; NUM_CEPS];
        for f in &frames {
            for k in 0..NUM_CEPS {
                mean[k] += f[k];
            }
        }
        for m in mean.iter_mut() {
            *m /= frames.len() as f32;
        }
        for f in frames.iter_mut() {
            for k in 0..NUM_CEPS {
                f[k] -= mean[k];
            }
        }
    }
    frames
}

/// Only the frames whose energy clears the noise floor, so silence does not
/// dominate the template.
pub fn voiced_frames(samples: &[f32]) -> Vec<Vec<f32>> {
    let all = mfcc(samples);
    if all.is_empty() {
        return all;
    }
    let energies: Vec<f32> = (0..)
        .map(|i| i * HOP_LEN)
        .take_while(|s| s + FRAME_LEN <= samples.len())
        .map(|s| frame_energy(&samples[s..s + FRAME_LEN]))
        .collect();
    let peak = energies.iter().cloned().fold(0.0f32, f32::max);
    if peak <= 1e-6 {
        return all;
    }
    let floor = peak * 0.12;
    let keep: Vec<Vec<f32>> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| energies.get(*i).copied().unwrap_or(0.0) >= floor)
        .map(|(_, f)| f.clone())
        .collect();
    if keep.len() >= 4 {
        keep
    } else {
        all
    }
}

/// Dynamic time warping between two MFCC sequences. Returns the length-
/// normalised Euclidean path cost, or `f32::INFINITY` for empty input.
pub fn dtw(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return f32::INFINITY;
    }
    let n = a.len();
    let m = b.len();
    let mut prev = vec![f32::INFINITY; m + 1];
    let mut cur = vec![f32::INFINITY; m + 1];
    prev[0] = 0.0;
    for i in 1..=n {
        cur[0] = f32::INFINITY;
        for j in 1..=m {
            let cost = euclid(&a[i - 1], &b[j - 1]);
            let best = prev[j].min(cur[j - 1]).min(prev[j - 1]);
            cur[j] = cost + if best.is_finite() { best } else { 0.0 };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m] / (n + m) as f32
}

fn euclid(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = 0.0f32;
    for k in 0..a.len().min(b.len()) {
        let d = a[k] - b[k];
        acc += d * d;
    }
    acc.sqrt()
}

/// Fixed-length speaker vector: mean and standard deviation of each cepstral
/// coefficient over the voiced frames, L2-normalised.
pub fn speaker_vector(samples: &[f32]) -> Vec<f32> {
    let frames = voiced_frames(samples);
    if frames.is_empty() {
        return Vec::new();
    }
    let n = frames.len() as f32;
    let mut mean = vec![0.0f32; NUM_CEPS];
    for f in &frames {
        for k in 0..NUM_CEPS {
            mean[k] += f[k];
        }
    }
    for m in mean.iter_mut() {
        *m /= n;
    }
    let mut var = vec![0.0f32; NUM_CEPS];
    for f in &frames {
        for k in 0..NUM_CEPS {
            let d = f[k] - mean[k];
            var[k] += d * d;
        }
    }
    let mut out = Vec::with_capacity(NUM_CEPS * 2);
    for k in 0..NUM_CEPS {
        out.push(mean[k]);
        out.push((var[k] / n).sqrt());
    }
    l2_normalise(&mut out);
    out
}

pub fn l2_normalise(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity in [-1, 1]. Used for the speaker check.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-9 || nb < 1e-9 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(-1.0, 1.0)
}

/// Average several speaker vectors into one template.
pub fn average_vectors(vectors: &[Vec<f32>]) -> Vec<f32> {
    let dim = vectors.iter().map(|v| v.len()).max().unwrap_or(0);
    if dim == 0 {
        return Vec::new();
    }
    let mut acc = vec![0.0f32; dim];
    let mut count = 0.0f32;
    for v in vectors {
        if v.len() != dim {
            continue;
        }
        for i in 0..dim {
            acc[i] += v[i];
        }
        count += 1.0;
    }
    if count > 0.0 {
        for a in acc.iter_mut() {
            *a /= count;
        }
    }
    l2_normalise(&mut acc);
    acc
}

/// Simple energy VAD: returns true when the window contains speech-like energy.
pub fn is_speech(window: &[f32], noise_floor: f32) -> bool {
    frame_energy(window) > noise_floor.max(1e-4) * 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, ms: u32, amp: f32) -> Vec<f32> {
        let n = (SAMPLE_RATE as f32 * ms as f32 / 1000.0) as usize;
        (0..n)
            .map(|i| {
                amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn mfcc_shape_is_stable() {
        let s = tone(220.0, 400, 0.5);
        let f = mfcc(&s);
        assert!(!f.is_empty());
        assert!(f.iter().all(|v| v.len() == NUM_CEPS));
        // Hop of 10ms over ~400ms gives roughly 40 frames.
        assert!(f.len() > 30 && f.len() < 50, "got {}", f.len());
    }

    #[test]
    fn identical_signals_have_zero_dtw() {
        let s = tone(180.0, 300, 0.4);
        let f = mfcc(&s);
        assert!(dtw(&f, &f) < 1e-3);
    }

    #[test]
    fn different_tones_are_further_apart() {
        let a = mfcc(&tone(180.0, 300, 0.4));
        let b = mfcc(&tone(180.0, 300, 0.4));
        let c = mfcc(&tone(900.0, 300, 0.4));
        assert!(dtw(&a, &b) < dtw(&a, &c));
    }

    #[test]
    fn speaker_vector_is_normalised_and_fixed_length() {
        let v = speaker_vector(&tone(240.0, 500, 0.5));
        assert_eq!(v.len(), NUM_CEPS * 2);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }

    #[test]
    fn cosine_self_is_one() {
        let v = speaker_vector(&tone(240.0, 400, 0.5));
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn empty_input_is_handled() {
        assert!(mfcc(&[]).is_empty());
        assert!(dtw(&[], &[]).is_infinite());
        assert_eq!(cosine(&[], &[]), 0.0);
        assert!(average_vectors(&[]).is_empty());
    }
}
