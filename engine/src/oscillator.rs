/// Waveform selected for every voice in an engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Saw,
    Square,
}

/// PolyBLEP band-limited oscillator (saw or square).
///
/// Naive digital saw/square waves create harsh extra frequencies (aliasing),
/// especially on high notes. PolyBLEP corrects the edges cheaply so the filter
/// and envelope are easier to hear.
pub struct Oscillator {
    phase: f32,
    phase_increment: f32,
    waveform: Waveform,
}

impl Oscillator {
    pub fn new(sample_rate_hz: f32, frequency_hz: f32, waveform: Waveform) -> Self {
        let mut osc = Self {
            phase: 0.0,
            phase_increment: 0.0,
            waveform,
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

    /// Advances one sample and returns a value in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.phase_increment;

        let sample = match self.waveform {
            Waveform::Saw => {
                // Naive saw in [-1, 1], then PolyBLEP at the wrap discontinuity.
                let mut value = 2.0 * t - 1.0;
                value -= poly_blep(t, dt);
                value
            }
            Waveform::Square => {
                // Naive square, PolyBLEP at rising (t=0) and falling (t=0.5) edges.
                let mut value = if t < 0.5 { 1.0 } else { -1.0 };
                value += poly_blep(t, dt);
                value -= poly_blep((t + 0.5) % 1.0, dt);
                value
            }
        };

        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample
    }
}

/// Polynomial BLEP correction near a rising discontinuity at phase 0.
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
