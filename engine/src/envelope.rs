/// Envelope stage. For the amp envelope, Idle means the voice is silent and free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Exponential-ish ADSR (0..1) used for amp, filter, and assignable envelopes.
///
/// Times are stored as per-sample coefficients derived from milliseconds.
/// Sustain is a level in 0..1 held until note-off.
pub struct Adsr {
    stage: EnvelopeStage,
    level: f32,
    attack_coeff: f32,
    decay_coeff: f32,
    release_coeff: f32,
    sustain: f32,
    sample_rate_hz: f32,
}

impl Adsr {
    const IDLE_LEVEL: f32 = 1.0e-4;
    /// Attack aims slightly above 1 so the asymptotic approach crosses 1 cleanly.
    const ATTACK_TARGET: f32 = 1.01;

    pub fn new(sample_rate_hz: f32) -> Self {
        let mut env = Self {
            stage: EnvelopeStage::Idle,
            level: 0.0,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            sustain: 0.7,
            sample_rate_hz,
        };
        env.set_times_ms(10.0, 100.0, 200.0);
        env
    }

    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.stage != EnvelopeStage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == EnvelopeStage::Release
    }

    pub fn set_sustain(&mut self, sustain: f32) {
        self.sustain = sustain.clamp(0.0, 1.0);
    }

    pub fn set_times_ms(&mut self, attack_ms: f32, decay_ms: f32, release_ms: f32) {
        self.attack_coeff = coeff_from_ms(self.sample_rate_hz, attack_ms);
        self.decay_coeff = coeff_from_ms(self.sample_rate_hz, decay_ms);
        self.release_coeff = coeff_from_ms(self.sample_rate_hz, release_ms);
    }

    pub fn note_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
        // Retrigger from current level so steals and overlaps do not hard-jump to 0.
    }

    pub fn note_off(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
        }
    }

    /// Advances one sample and returns the current level in 0..1.
    pub fn next_level(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => {
                self.level = 0.0;
            }
            EnvelopeStage::Attack => {
                self.level += (Self::ATTACK_TARGET - self.level) * self.attack_coeff;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.level += (self.sustain - self.level) * self.decay_coeff;
                if (self.level - self.sustain).abs() < Self::IDLE_LEVEL {
                    self.level = self.sustain;
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => {
                self.level = self.sustain;
            }
            EnvelopeStage::Release => {
                self.level += (0.0 - self.level) * self.release_coeff;
                if self.level <= Self::IDLE_LEVEL {
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        self.level
    }
}

/// Attack, decay, sustain, and release for one ADSR (amp, filter, or assignable).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrTimes {
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
}

impl Default for AdsrTimes {
    fn default() -> Self {
        Self {
            attack_ms: 10.0,
            decay_ms: 100.0,
            sustain: 0.7,
            release_ms: 200.0,
        }
    }
}

impl AdsrTimes {
    pub(crate) fn clamped(self) -> Self {
        Self {
            attack_ms: self.attack_ms.max(0.0),
            decay_ms: self.decay_ms.max(0.0),
            sustain: self.sustain.clamp(0.0, 1.0),
            release_ms: self.release_ms.max(0.0),
        }
    }

    pub(crate) fn apply_to(self, adsr: &mut Adsr) {
        adsr.set_times_ms(self.attack_ms, self.decay_ms, self.release_ms);
        adsr.set_sustain(self.sustain);
    }
}

fn coeff_from_ms(sample_rate_hz: f32, time_ms: f32) -> f32 {
    // Very short times still need a usable coefficient so the stage can finish.
    let time_samples = (time_ms.max(0.1) * 0.001) * sample_rate_hz;
    1.0 - libm::expf(-1.0 / time_samples)
}

/// Maps MIDI velocity 0..127 to an amplitude scale with a simple square curve.
/// Softer hits drop more than a straight `velocity / 127` line.
pub fn velocity_to_amp(velocity: u8) -> f32 {
    let v = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    v * v
}
