use core::f32::consts::PI;

/// Per-voice state-variable filter (SVF), lowpass output.
///
/// Cutoff is the frequency above which brightness is turned down.
/// Resonance (0–1) boosts energy near that cutoff.
pub struct Svf {
    ic1eq: f32,
    ic2eq: f32,
}

impl Svf {
    pub fn new() -> Self {
        Self {
            ic1eq: 0.0,
            ic2eq: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    /// Processes one sample. `cutoff_hz` and `resonance` are per-sample values (params plus envelope modulation).
    pub fn process(
        &mut self,
        input: f32,
        sample_rate_hz: f32,
        cutoff_hz: f32,
        resonance: f32,
    ) -> f32 {
        let nyquist = sample_rate_hz * 0.5;
        let cutoff = cutoff_hz.clamp(20.0, nyquist * 0.99);
        let res = resonance.clamp(0.0, 1.0);

        // Map resonance 0..1 into a useful Q range (0.5 .. ~20).
        let q = 0.5 * libm::expf(res * 3.7);
        let g = libm::tanf(PI * cutoff / sample_rate_hz);
        let k = 1.0 / q;

        // Andy Simper / Cytomic linear trapezoidal SVF (lowpass = v2).
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v3 = input - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        v2
    }
}

impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;
    const ANALYSIS_SAMPLES: usize = 4096;

    fn sine_sample(phase: &mut f32, freq_hz: f32, sr: f32) -> f32 {
        *phase += freq_hz / sr;
        if *phase >= 1.0 {
            *phase -= 1.0;
        }
        libm::sinf(core::f32::consts::TAU * *phase)
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        libm::sqrtf(sum_sq / samples.len() as f32)
    }

    fn filter_sine(cutoff_hz: f32, resonance: f32, freq_hz: f32) -> Vec<f32> {
        let mut svf = Svf::new();
        let mut phase = 0.0f32;
        // Warm up so transient state does not dominate the measurement.
        for _ in 0..2_000 {
            let input = sine_sample(&mut phase, freq_hz, SAMPLE_RATE_HZ);
            let _ = svf.process(input, SAMPLE_RATE_HZ, cutoff_hz, resonance);
        }
        (0..ANALYSIS_SAMPLES)
            .map(|_| {
                let input = sine_sample(&mut phase, freq_hz, SAMPLE_RATE_HZ);
                svf.process(input, SAMPLE_RATE_HZ, cutoff_hz, resonance)
            })
            .collect()
    }

    #[test]
    fn low_cutoff_attenuates_high_frequency() {
        let freq_hz = 2_000.0;
        let dark = filter_sine(500.0, 0.0, freq_hz);
        let bright = filter_sine(10_000.0, 0.0, freq_hz);
        let dark_rms = rms(&dark);
        let bright_rms = rms(&bright);
        assert!(
            bright_rms > dark_rms * 2.0,
            "open cutoff should pass more 2 kHz energy; dark={dark_rms} bright={bright_rms}"
        );
    }

    #[test]
    fn high_resonance_boosts_near_cutoff() {
        let cutoff_hz = 1_000.0;
        let quiet = filter_sine(cutoff_hz, 0.0, cutoff_hz);
        let resonant = filter_sine(cutoff_hz, 0.8, cutoff_hz);
        let quiet_peak = quiet.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
        let resonant_peak = resonant.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
        assert!(
            resonant_peak > quiet_peak * 1.5,
            "higher resonance should boost near cutoff; quiet={quiet_peak} resonant={resonant_peak}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut svf = Svf::new();
        // Build internal state with a sustained tone.
        let mut phase = 0.0f32;
        for _ in 0..2_000 {
            let input = sine_sample(&mut phase, 440.0, SAMPLE_RATE_HZ);
            let _ = svf.process(input, SAMPLE_RATE_HZ, 1_000.0, 0.5);
        }
        svf.reset();

        // After reset, an impulse response should start from silence (near-zero first output
        // on zero input), matching a fresh filter.
        let mut fresh = Svf::new();
        let after_reset = svf.process(0.0, SAMPLE_RATE_HZ, 1_000.0, 0.5);
        let fresh_out = fresh.process(0.0, SAMPLE_RATE_HZ, 1_000.0, 0.5);
        assert!(
            (after_reset - fresh_out).abs() < 1e-6,
            "reset should match a new filter on zero input; after={after_reset} fresh={fresh_out}"
        );

        let impulse_reset = svf.process(1.0, SAMPLE_RATE_HZ, 1_000.0, 0.5);
        let impulse_fresh = fresh.process(1.0, SAMPLE_RATE_HZ, 1_000.0, 0.5);
        assert!(
            (impulse_reset - impulse_fresh).abs() < 1e-6,
            "impulse after reset should match a fresh filter"
        );
    }

    #[test]
    fn cutoff_clamps_at_nyquist() {
        let mut svf = Svf::new();
        let mut phase = 0.0f32;
        for _ in 0..ANALYSIS_SAMPLES {
            let input = sine_sample(&mut phase, 440.0, SAMPLE_RATE_HZ);
            let out = svf.process(input, SAMPLE_RATE_HZ, 100_000.0, 0.5);
            assert!(
                out.is_finite(),
                "extreme cutoff must not produce NaN/inf; got {out}"
            );
        }
    }
}
