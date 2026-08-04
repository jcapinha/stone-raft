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

    /// Processes one sample. `cutoff_hz` and `resonance` come from shared engine params.
    pub fn process(&mut self, input: f32, sample_rate_hz: f32, cutoff_hz: f32, resonance: f32) -> f32 {
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
