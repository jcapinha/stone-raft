#![cfg_attr(not(test), no_std)]

use core::f32::consts::TAU;

/// A single sine-wave generator.
///
/// `core` (the no_std subset of the standard library) has no trig functions,
/// since `sin`/`cos` normally come from the platform's libm. `libm` is a pure
/// Rust reimplementation used here so this code can run with no operating
/// system underneath, as it will on the Daisy Seed.
pub struct Oscillator {
    /// Position within one cycle, kept in the range [0.0, 1.0).
    phase: f32,
    /// How far `phase` advances per sample, derived from frequency and sample rate.
    phase_increment: f32,
}

impl Oscillator {
    pub fn new(sample_rate_hz: f32, frequency_hz: f32) -> Self {
        let mut osc = Self {
            phase: 0.0,
            phase_increment: 0.0,
        };
        osc.set_frequency(sample_rate_hz, frequency_hz);
        osc
    }

    pub fn set_frequency(&mut self, sample_rate_hz: f32, frequency_hz: f32) {
        self.phase_increment = frequency_hz / sample_rate_hz;
    }

    /// Advances the oscillator by one sample and returns its value in [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        let sample = libm::sinf(self.phase * TAU);
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;
    // Chosen so the period is a round number of samples, which keeps the math in these tests simple.
    const FREQUENCY_HZ: f32 = 480.0;
    const SAMPLES_PER_PERIOD: usize = (SAMPLE_RATE_HZ / FREQUENCY_HZ) as usize;

    #[test]
    fn stays_within_unit_range() {
        let mut osc = Oscillator::new(SAMPLE_RATE_HZ, FREQUENCY_HZ);
        for _ in 0..10_000 {
            let sample = osc.next_sample();
            assert!(
                (-1.0..=1.0).contains(&sample),
                "sample {sample} escaped [-1.0, 1.0]"
            );
        }
    }

    #[test]
    fn repeats_after_one_period() {
        let mut osc = Oscillator::new(SAMPLE_RATE_HZ, FREQUENCY_HZ);
        let first = osc.next_sample();
        for _ in 1..SAMPLES_PER_PERIOD {
            osc.next_sample();
        }
        let after_one_period = osc.next_sample();

        let diff = (after_one_period - first).abs();
        assert!(
            diff < 0.01,
            "expected the wave to repeat after one period, diff was {diff}"
        );
    }

    #[test]
    fn reaches_peak_a_quarter_period_in() {
        let mut osc = Oscillator::new(SAMPLE_RATE_HZ, FREQUENCY_HZ);
        let mut sample = 0.0;
        for _ in 0..(SAMPLES_PER_PERIOD / 4) {
            sample = osc.next_sample();
        }
        assert!(
            sample > 0.95,
            "expected close to the peak (1.0) a quarter period in, got {sample}"
        );
    }
}
