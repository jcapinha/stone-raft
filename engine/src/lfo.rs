//! Per-voice assignable LFO: phase, five waves, and sample-and-hold.

use crate::AssignableDest;

/// LFO rate in Hz. Below this the motion is too slow to hear as a cycle.
pub const LFO_RATE_MIN_HZ: f32 = 0.05;
/// LFO rate in Hz. Above this the motion is no longer a slow modulator.
pub const LFO_RATE_MAX_HZ: f32 = 20.0;
/// Default LFO rate in Hz.
pub const LFO_RATE_DEFAULT_HZ: f32 = 1.0;

/// Which of the two assignable LFOs on an engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoId {
    One,
    Two,
}

impl LfoId {
    pub fn index(self) -> usize {
        match self {
            LfoId::One => 0,
            LfoId::Two => 1,
        }
    }
}

/// Wave shape for an assignable LFO. Levels are bipolar (-1..1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWave {
    Sine,
    Triangle,
    Square,
    Saw,
    SampleHold,
}

/// Shared settings for one assignable LFO. Per engine instance, used by every voice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LfoParams {
    pub dest: AssignableDest,
    pub amount: f32,
    pub rate_hz: f32,
    pub wave: LfoWave,
    pub retrigger: bool,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            dest: AssignableDest::Off,
            amount: 0.0,
            rate_hz: LFO_RATE_DEFAULT_HZ,
            wave: LfoWave::Sine,
            retrigger: true,
        }
    }
}

/// Per-voice LFO runner. Reads dest, amount, rate, and wave from [`LfoParams`] each sample.
pub(crate) struct Lfo {
    sample_rate_hz: f32,
    phase: f32,
    held: f32,
    rng: u32,
}

impl Lfo {
    pub(crate) fn new(sample_rate_hz: f32, voice_index: usize, lfo_index: usize) -> Self {
        Self {
            sample_rate_hz,
            phase: 0.0,
            held: 0.0,
            rng: mix_seed(voice_index, lfo_index),
        }
    }

    /// Reset phase to 0. Sample-and-hold draws a new value immediately.
    pub(crate) fn retrigger(&mut self) {
        self.phase = 0.0;
        self.held = self.next_bipolar();
    }

    /// Advances one sample and returns a level in -1..1.
    pub(crate) fn next_level(&mut self, rate_hz: f32, wave: LfoWave) -> f32 {
        let increment = rate_hz / self.sample_rate_hz;
        let level = match wave {
            LfoWave::Sine => libm::sinf(core::f32::consts::TAU * self.phase),
            LfoWave::Triangle => triangle_level(self.phase),
            LfoWave::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::Saw => 2.0 * self.phase - 1.0,
            LfoWave::SampleHold => self.held,
        };

        self.phase += increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            if matches!(wave, LfoWave::SampleHold) {
                self.held = self.next_bipolar();
            }
        }
        level
    }

    fn next_bipolar(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / (u32::MAX as f32)) * 2.0 - 1.0
    }
}

/// Triangle: 0 at phase 0, +1 at 0.25, 0 at 0.5, -1 at 0.75, back to 0.
fn triangle_level(phase: f32) -> f32 {
    if phase < 0.25 {
        phase * 4.0
    } else if phase < 0.75 {
        2.0 - phase * 4.0
    } else {
        phase * 4.0 - 4.0
    }
}

fn mix_seed(voice_index: usize, lfo_index: usize) -> u32 {
    let voice = (voice_index as u32).wrapping_add(1);
    let lfo = (lfo_index as u32).wrapping_add(1);
    let mut z = voice.wrapping_mul(0x9E37_79B9) ^ lfo.wrapping_mul(0x85EB_CA6B);
    z = (z ^ (z >> 16)).wrapping_mul(0x7FEB_352D);
    z = (z ^ (z >> 15)).wrapping_mul(0x846C_A68B);
    let mixed = z ^ (z >> 16);
    if mixed == 0 { 1 } else { mixed }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;

    fn lfo() -> Lfo {
        Lfo::new(SAMPLE_RATE_HZ, 0, 0)
    }

    #[test]
    fn sine_stays_in_unit_bipolar_range() {
        let mut lfo = lfo();
        lfo.retrigger();
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for _ in 0..SAMPLE_RATE_HZ as usize {
            let level = lfo.next_level(1.0, LfoWave::Sine);
            assert!((-1.0..=1.0).contains(&level), "sine left -1..1: {level}");
            min = min.min(level);
            max = max.max(level);
        }
        assert!(min < -0.99, "sine should reach near -1, min={min}");
        assert!(max > 0.99, "sine should reach near +1, max={max}");
    }

    #[test]
    fn square_starts_positive_after_retrigger() {
        let mut lfo = lfo();
        lfo.retrigger();
        assert_eq!(lfo.next_level(1.0, LfoWave::Square), 1.0);
    }

    #[test]
    fn saw_rises_after_retrigger() {
        let mut lfo = lfo();
        lfo.retrigger();
        let first = lfo.next_level(1.0, LfoWave::Saw);
        let second = lfo.next_level(1.0, LfoWave::Saw);
        assert!(
            first < 0.0,
            "rising saw should start below 0 after retrigger, got {first}"
        );
        assert!(
            second > first,
            "rising saw should increase; first={first} second={second}"
        );
    }

    #[test]
    fn sample_and_hold_holds_then_jumps() {
        let mut lfo = lfo();
        lfo.retrigger();
        // 480 Hz at 48 kHz: one cycle every 100 samples.
        let rate_hz = 480.0;
        let held = lfo.next_level(rate_hz, LfoWave::SampleHold);
        for _ in 0..90 {
            assert_eq!(
                lfo.next_level(rate_hz, LfoWave::SampleHold),
                held,
                "sample-and-hold should keep the same value before wrap"
            );
        }
        let mut jumped = held;
        for _ in 0..20 {
            jumped = lfo.next_level(rate_hz, LfoWave::SampleHold);
        }
        assert_ne!(
            jumped, held,
            "sample-and-hold should draw a new value after wrap"
        );
    }

    #[test]
    fn rate_constants_match_the_documented_range() {
        assert!((LFO_RATE_MIN_HZ - 0.05).abs() < f32::EPSILON);
        assert!((LFO_RATE_MAX_HZ - 20.0).abs() < f32::EPSILON);
        assert!((LFO_RATE_DEFAULT_HZ - 1.0).abs() < f32::EPSILON);
        assert!((LfoParams::default().rate_hz - LFO_RATE_DEFAULT_HZ).abs() < f32::EPSILON);
    }
}
