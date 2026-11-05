//! BLACKVAULT — energy-based voice activity detection and utterance capture.
//!
//! The engine feeds fixed 20 ms blocks in. The detector adapts a noise floor
//! while idle, opens on a sustained rise above it, and closes after a run of
//! quiet blocks. It returns whole utterances, which is what the matcher wants.

use crate::dsp::frame_energy;

pub const BLOCK_MS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Idle,
    Speech,
}

pub struct Vad {
    state: VadState,
    noise_floor: f32,
    speech_blocks: usize,
    quiet_blocks: usize,
    /// Blocks of speech required before we believe it (debounce).
    min_speech: usize,
    /// Blocks of quiet required before we close the utterance.
    min_quiet: usize,
}

impl Default for Vad {
    fn default() -> Self {
        Vad {
            state: VadState::Idle,
            noise_floor: 1e-4,
            speech_blocks: 0,
            quiet_blocks: 0,
            min_speech: 3,
            // ~800 ms of quiet before the utterance is closed. The old 240 ms
            // split a phrase on any natural pause ("lock … this pc"), so the
            // two halves were recognised separately and no command ever matched.
            min_quiet: 40,
        }
    }
}

impl Vad {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_min_quiet_blocks(&mut self, n: usize) {
        self.min_quiet = n.max(2);
    }

    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    /// Feed one block. Returns true when speech is ongoing, false when quiet.
    pub fn push(&mut self, block: &[f32]) -> bool {
        let energy = frame_energy(block);
        match self.state {
            VadState::Idle => {
                // Adapt the floor downwards quickly, upwards slowly, and never
                // let a loud burst poison it permanently.
                if energy < self.noise_floor {
                    self.noise_floor = 0.9 * self.noise_floor + 0.1 * energy;
                } else {
                    self.noise_floor = 0.995 * self.noise_floor + 0.005 * energy;
                }
                if energy > self.noise_floor * 3.5 && energy > 2e-3 {
                    self.speech_blocks += 1;
                    if self.speech_blocks >= self.min_speech {
                        self.state = VadState::Speech;
                        self.quiet_blocks = 0;
                        return true;
                    }
                } else {
                    self.speech_blocks = 0;
                }
                false
            }
            VadState::Speech => {
                if energy > self.noise_floor * 2.0 {
                    self.quiet_blocks = 0;
                    true
                } else {
                    self.quiet_blocks += 1;
                    if self.quiet_blocks >= self.min_quiet {
                        self.state = VadState::Idle;
                        self.speech_blocks = 0;
                        false
                    } else {
                        true
                    }
                }
            }
        }
    }

    pub fn in_speech(&self) -> bool {
        self.state == VadState::Speech
    }

    pub fn reset(&mut self) {
        self.state = VadState::Idle;
        self.speech_blocks = 0;
        self.quiet_blocks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(n: usize) -> Vec<f32> {
        vec![0.0005; n]
    }
    fn loud(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.4 * (i as f32 * 0.3).sin()).collect()
    }

    #[test]
    fn opens_on_sustained_speech() {
        let mut v = Vad::new();
        for _ in 0..20 {
            v.push(&quiet(320));
        }
        assert!(!v.in_speech());
        let mut opened = false;
        for _ in 0..6 {
            if v.push(&loud(320)) {
                opened = true;
            }
        }
        assert!(opened);
        assert!(v.in_speech());
    }

    #[test]
    fn closes_after_quiet_run() {
        let mut v = Vad::new();
        for _ in 0..10 {
            v.push(&quiet(320));
        }
        for _ in 0..6 {
            v.push(&loud(320));
        }
        assert!(v.in_speech());
        for _ in 0..40 {
            v.push(&quiet(320));
        }
        assert!(!v.in_speech());
    }

    #[test]
    fn reset_returns_to_idle() {
        let mut v = Vad::new();
        for _ in 0..10 {
            v.push(&loud(320));
        }
        v.reset();
        assert!(!v.in_speech());
    }
}
