use crate::sine::sine_from_phase;

/// Waveform selected for every voice in an engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Saw,
    Square,
    Triangle,
    Sine,
}

/// Minimum / maximum pulse width so a square never collapses to silence or DC.
pub const PULSE_WIDTH_MIN: f32 = 0.05;
pub const PULSE_WIDTH_MAX: f32 = 0.95;
pub const PULSE_WIDTH_DEFAULT: f32 = 0.5;

/// Band-limited oscillator: PolyBLEP saw/square, PolyBLAMP triangle, table sine.
///
/// Naive digital saw/square waves create harsh extra frequencies (aliasing),
/// especially on high notes. PolyBLEP corrects value jumps at edges; PolyBLAMP
/// corrects slope jumps at triangle corners. Sine has no sharp edges, so it
/// reads the shared lookup table instead of calling `libm::sinf` per sample.
pub struct Oscillator {
    phase: f32,
    phase_increment: f32,
    waveform: Waveform,
    pulse_width: f32,
}

impl Oscillator {
    pub fn new(sample_rate_hz: f32, frequency_hz: f32, waveform: Waveform) -> Self {
        let mut osc = Self {
            phase: 0.0,
            phase_increment: 0.0,
            waveform,
            pulse_width: PULSE_WIDTH_DEFAULT,
        };
        osc.set_frequency(sample_rate_hz, frequency_hz);
        osc
    }

    pub fn set_frequency(&mut self, sample_rate_hz: f32, frequency_hz: f32) {
        self.phase_increment = frequency_hz / sample_rate_hz;
    }

    pub fn set_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
    }

    pub fn set_pulse_width(&mut self, width: f32) {
        self.pulse_width = width.clamp(PULSE_WIDTH_MIN, PULSE_WIDTH_MAX);
    }

    /// Advances one sample and returns a value in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        match self.waveform {
            Waveform::Saw => self.next_saw(),
            Waveform::Square => self.next_square(),
            Waveform::Triangle => self.next_triangle(),
            Waveform::Sine => self.next_sine(),
        }
    }

    #[inline(always)]
    pub(crate) fn next_saw(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.phase_increment;
        let mut value = 2.0 * t - 1.0;
        value -= poly_blep(t, dt);
        self.advance_phase();
        value
    }

    #[inline(always)]
    pub(crate) fn next_square(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.phase_increment;
        let pw = self.pulse_width;
        let mut value = if t < pw { 1.0 } else { -1.0 };
        value += poly_blep(t, dt);
        value -= poly_blep((t + (1.0 - pw)) % 1.0, dt);
        self.advance_phase();
        value
    }

    #[inline(always)]
    pub(crate) fn next_triangle(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.phase_increment;
        let mut value = if t < 0.5 {
            4.0 * t - 1.0
        } else {
            3.0 - 4.0 * t
        };
        value += 8.0 * dt * poly_blamp(t, dt);
        value -= 8.0 * dt * poly_blamp((t + 0.5) % 1.0, dt);
        self.advance_phase();
        value
    }

    #[inline(always)]
    pub(crate) fn next_sine(&mut self) -> f32 {
        let value = sine_from_phase(self.phase);
        self.advance_phase();
        value
    }

    #[inline(always)]
    fn advance_phase(&mut self) {
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }
}

/// Polynomial BLEP correction near a rising discontinuity at phase 0.
#[inline(always)]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// Polynomial BLAMP correction near a unit slope discontinuity at phase 0.
///
/// PolyBLEP fixes a jump in level; PolyBLAMP fixes a jump in slope (as at a
/// triangle corner). Multiply by the size of the slope change (and by `dt`
/// when using this dimensionless residual form).
#[inline(always)]
fn poly_blamp(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let t = t / dt - 1.0;
        -(t * t * t) / 3.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt + 1.0;
        (t * t * t) / 3.0
    } else {
        0.0
    }
}
