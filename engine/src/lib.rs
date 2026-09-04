#![cfg_attr(not(test), no_std)]

mod envelope;
mod filter;
mod lfo;
mod mixer;
mod oscillator;
mod voices;

pub use envelope::{Adsr, AdsrTimes, EnvelopeStage, velocity_to_amp};
pub use filter::Svf;
pub use lfo::{LFO_RATE_DEFAULT_HZ, LFO_RATE_MAX_HZ, LFO_RATE_MIN_HZ, LfoId, LfoParams, LfoWave};
pub use mixer::{ENGINE_COUNT, InstanceEvent, Mixer, MixerEvent};
pub use oscillator::{Oscillator, PULSE_WIDTH_DEFAULT, PULSE_WIDTH_MAX, PULSE_WIDTH_MIN, Waveform};

use voices::Voices;

/// Fixed number of simultaneous voices per engine instance.
pub const VOICE_COUNT: usize = 4;

const AMT_MIN: f32 = -8.0;
const AMT_MAX: f32 = 8.0;

/// Destination for an assignable envelope or LFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignableDest {
    Off,
    Resonance,
    Pitch,
    Cutoff,
    PulseWidth,
    Amp,
}

/// How many octaves below the sounding pitch the sub oscillator sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubOctaves {
    One,
    Two,
}

impl SubOctaves {
    /// Frequency divisor: one octave = /2, two octaves = /4.
    pub fn frequency_divisor(self) -> f32 {
        match self {
            SubOctaves::One => 2.0,
            SubOctaves::Two => 4.0,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            SubOctaves::One => 1,
            SubOctaves::Two => 2,
        }
    }
}

/// Which of the three ADSRs on a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeId {
    Amp,
    Filter,
    Assignable,
}

/// One ADSR time or sustain field, used with [`ControlEvent::PatchEnvelope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeField {
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Discrete notes and param changes. Hosts enqueue these; only the audio thread calls [`Engine::apply`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlEvent {
    NoteOn {
        note: u8,
        velocity: u8,
    },
    NoteOff {
        note: u8,
    },
    SetCutoff {
        hz: f32,
    },
    SetResonance {
        amount: f32,
    },
    SetWave {
        waveform: Waveform,
    },
    SetSawVol {
        amount: f32,
    },
    SetSquareVol {
        amount: f32,
    },
    SetTriangleVol {
        amount: f32,
    },
    SetSineVol {
        amount: f32,
    },
    SetPulse {
        width: f32,
    },
    SetEnvelope {
        which: EnvelopeId,
        times: AdsrTimes,
    },
    PatchEnvelope {
        which: EnvelopeId,
        field: EnvelopeField,
        value: f32,
    },
    SetFilterEnvAmount {
        amount: f32,
    },
    SetAssignableDest {
        dest: AssignableDest,
    },
    SetAssignableAmount {
        amount: f32,
    },
    SetLfoDest {
        which: LfoId,
        dest: AssignableDest,
    },
    SetLfoAmount {
        which: LfoId,
        amount: f32,
    },
    SetLfoRate {
        which: LfoId,
        rate_hz: f32,
    },
    SetLfoWave {
        which: LfoId,
        wave: LfoWave,
    },
    SetLfoRetrig {
        which: LfoId,
        on: bool,
    },
    EnvCopy,
    SetEnvLink {
        on: bool,
    },
    SetEnvVel {
        amount: f32,
    },
    SetSubVol {
        amount: f32,
    },
    SetSubOct {
        octaves: SubOctaves,
    },
}

/// Converts a MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
pub fn midi_note_to_hz(note: u8) -> f32 {
    let semitones_from_a4 = f32::from(note) - 69.0;
    440.0 * libm::powf(2.0, semitones_from_a4 / 12.0)
}

pub(crate) fn hz_times_octaves(hz: f32, octaves: f32) -> f32 {
    hz * libm::powf(2.0, octaves)
}

/// Live voice updates required after applying a parameter event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamEffects {
    /// Copy the stored pulse width to square oscillators on existing voices.
    pub synchronize_pulse_width: bool,
    /// Copy the stored ADSR settings to existing voices.
    pub synchronize_envelopes: bool,
}

impl ParamEffects {
    const NONE: Self = Self {
        synchronize_pulse_width: false,
        synchronize_envelopes: false,
    };
    const PULSE_WIDTH: Self = Self {
        synchronize_pulse_width: true,
        synchronize_envelopes: false,
    };
    const ENVELOPES: Self = Self {
        synchronize_pulse_width: false,
        synchronize_envelopes: true,
    };
}

/// Shared subtractive params for one engine instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineParams {
    pub saw_vol: f32,
    pub square_vol: f32,
    pub triangle_vol: f32,
    pub sine_vol: f32,
    pub pulse_width: f32,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub amp_env: AdsrTimes,
    pub filter_env: AdsrTimes,
    pub assignable_env: AdsrTimes,
    pub filter_env_amount: f32,
    pub assignable_amount: f32,
    pub assignable_dest: AssignableDest,
    pub env_link: bool,
    pub env_vel: f32,
    pub sub_vol: f32,
    pub sub_octaves: SubOctaves,
    pub lfos: [LfoParams; 2],
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            saw_vol: 1.0,
            square_vol: 0.0,
            triangle_vol: 0.0,
            sine_vol: 0.0,
            pulse_width: PULSE_WIDTH_DEFAULT,
            cutoff_hz: 2_000.0,
            resonance: 0.2,
            amp_env: AdsrTimes::default(),
            filter_env: AdsrTimes::default(),
            assignable_env: AdsrTimes::default(),
            filter_env_amount: 0.0,
            assignable_amount: 0.0,
            assignable_dest: AssignableDest::Off,
            env_link: false,
            env_vel: 0.0,
            sub_vol: 0.0,
            sub_octaves: SubOctaves::One,
            lfos: [LfoParams::default(); 2],
        }
    }
}

impl EngineParams {
    pub fn envelope(&self, which: EnvelopeId) -> AdsrTimes {
        match which {
            EnvelopeId::Amp => self.amp_env,
            EnvelopeId::Filter => self.filter_env,
            EnvelopeId::Assignable => self.assignable_env,
        }
    }

    /// Updates params and reports live voice synchronization required by param events.
    ///
    /// Note events return `None` because they do not update shared parameters.
    pub fn apply(&mut self, event: ControlEvent) -> Option<ParamEffects> {
        let effects = match event {
            ControlEvent::NoteOn { .. } | ControlEvent::NoteOff { .. } => return None,
            ControlEvent::SetCutoff { hz } => {
                self.cutoff_hz = hz.max(20.0);
                ParamEffects::NONE
            }
            ControlEvent::SetResonance { amount } => {
                self.resonance = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetWave { waveform } => {
                self.saw_vol = 0.0;
                self.square_vol = 0.0;
                self.triangle_vol = 0.0;
                self.sine_vol = 0.0;
                self.sub_vol = 0.0;
                match waveform {
                    Waveform::Saw => self.saw_vol = 1.0,
                    Waveform::Square => self.square_vol = 1.0,
                    Waveform::Triangle => self.triangle_vol = 1.0,
                    Waveform::Sine => self.sine_vol = 1.0,
                }
                ParamEffects::NONE
            }
            ControlEvent::SetSawVol { amount } => {
                self.saw_vol = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetSquareVol { amount } => {
                self.square_vol = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetTriangleVol { amount } => {
                self.triangle_vol = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetSineVol { amount } => {
                self.sine_vol = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetPulse { width } => {
                self.pulse_width = width.clamp(PULSE_WIDTH_MIN, PULSE_WIDTH_MAX);
                ParamEffects::PULSE_WIDTH
            }
            ControlEvent::SetEnvelope { which, times } => {
                self.set_envelope(which, times);
                ParamEffects::ENVELOPES
            }
            ControlEvent::PatchEnvelope {
                which,
                field,
                value,
            } => {
                let mut times = self.envelope(which);
                match field {
                    EnvelopeField::Attack => times.attack_ms = value,
                    EnvelopeField::Decay => times.decay_ms = value,
                    EnvelopeField::Sustain => times.sustain = value,
                    EnvelopeField::Release => times.release_ms = value,
                }
                self.set_envelope(which, times);
                ParamEffects::ENVELOPES
            }
            ControlEvent::SetFilterEnvAmount { amount } => {
                self.filter_env_amount = amount.clamp(AMT_MIN, AMT_MAX);
                ParamEffects::NONE
            }
            ControlEvent::SetAssignableDest { dest } => {
                self.assignable_dest = dest;
                ParamEffects::NONE
            }
            ControlEvent::SetAssignableAmount { amount } => {
                self.assignable_amount = amount.clamp(AMT_MIN, AMT_MAX);
                ParamEffects::NONE
            }
            ControlEvent::SetLfoDest { which, dest } => {
                self.lfos[which.index()].dest = dest;
                ParamEffects::NONE
            }
            ControlEvent::SetLfoAmount { which, amount } => {
                self.lfos[which.index()].amount = amount.clamp(AMT_MIN, AMT_MAX);
                ParamEffects::NONE
            }
            ControlEvent::SetLfoRate { which, rate_hz } => {
                self.lfos[which.index()].rate_hz = rate_hz.clamp(LFO_RATE_MIN_HZ, LFO_RATE_MAX_HZ);
                ParamEffects::NONE
            }
            ControlEvent::SetLfoWave { which, wave } => {
                self.lfos[which.index()].wave = wave;
                ParamEffects::NONE
            }
            ControlEvent::SetLfoRetrig { which, on } => {
                self.lfos[which.index()].retrigger = on;
                ParamEffects::NONE
            }
            ControlEvent::EnvCopy => {
                self.copy_amp_times_to_extra_envelopes();
                ParamEffects::ENVELOPES
            }
            ControlEvent::SetEnvLink { on } => {
                self.env_link = on;
                if on {
                    self.copy_amp_times_to_extra_envelopes();
                    ParamEffects::ENVELOPES
                } else {
                    ParamEffects::NONE
                }
            }
            ControlEvent::SetEnvVel { amount } => {
                self.env_vel = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetSubVol { amount } => {
                self.sub_vol = amount.clamp(0.0, 1.0);
                ParamEffects::NONE
            }
            ControlEvent::SetSubOct { octaves } => {
                self.sub_octaves = octaves;
                ParamEffects::NONE
            }
        };
        Some(effects)
    }

    fn set_envelope(&mut self, which: EnvelopeId, times: AdsrTimes) {
        let times = times.clamped();
        match which {
            EnvelopeId::Amp => {
                self.amp_env = times;
                if self.env_link {
                    self.filter_env = times;
                    self.assignable_env = times;
                }
            }
            EnvelopeId::Filter => {
                self.unlink_if_needed();
                self.filter_env = times;
            }
            EnvelopeId::Assignable => {
                self.unlink_if_needed();
                self.assignable_env = times;
            }
        }
    }

    fn unlink_if_needed(&mut self) {
        if self.env_link {
            self.env_link = false;
        }
    }

    fn copy_amp_times_to_extra_envelopes(&mut self) {
        self.filter_env = self.amp_env;
        self.assignable_env = self.amp_env;
    }
}

/// One engine instance: fixed voices that turn MIDI notes into mono audio.
pub struct Engine {
    voices: Voices,
    params: EngineParams,
    #[cfg(test)]
    next_sample_calls: u32,
}

impl Engine {
    pub fn new(sample_rate_hz: f32) -> Self {
        let params = EngineParams::default();
        Self {
            voices: Voices::new(sample_rate_hz, &params),
            params,
            #[cfg(test)]
            next_sample_calls: 0,
        }
    }

    pub fn params(&self) -> &EngineParams {
        &self.params
    }

    /// Applies one note or param change. Call only from the audio thread.
    pub fn apply(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::NoteOn { note, velocity } => {
                self.voices.note_on(note, velocity, &self.params)
            }
            ControlEvent::NoteOff { note } => self.voices.note_off(note),
            _ => {
                if let Some(effects) = self.params.apply(event) {
                    if effects.synchronize_pulse_width {
                        self.voices.synchronize_pulse_width(self.params.pulse_width);
                    }
                    if effects.synchronize_envelopes {
                        self.voices.synchronize_envelopes(&self.params);
                    }
                }
            }
        }
    }

    /// Force every voice envelope to Idle at level 0 so sound does not resume later.
    pub fn silence(&mut self) {
        self.voices.silence();
    }

    #[cfg(test)]
    pub(crate) fn next_sample_call_count(&self) -> u32 {
        self.next_sample_calls
    }

    /// Sums active voices into one mono sample in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        #[cfg(test)]
        {
            self.next_sample_calls = self.next_sample_calls.wrapping_add(1);
        }
        self.voices.render_sample(&self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;
    const ANALYSIS_SAMPLES: usize = 4096;
    const TONE_PRESENT: f32 = 0.02;
    const TONE_ABSENT: f32 = 0.01;

    fn take_samples(engine: &mut Engine, count: usize) -> Vec<f32> {
        (0..count).map(|_| engine.next_sample()).collect()
    }

    /// How strongly `frequency_hz` appears in `samples` (normalized DFT bin magnitude).
    fn tone_strength(samples: &[f32], frequency_hz: f32) -> f32 {
        let mut re = 0.0f32;
        let mut im = 0.0f32;
        for (n, &sample) in samples.iter().enumerate() {
            let phase = core::f32::consts::TAU * frequency_hz * (n as f32) / SAMPLE_RATE_HZ;
            re += sample * libm::cosf(phase);
            im += sample * libm::sinf(phase);
        }
        (re * re + im * im).sqrt() / samples.len() as f32
    }

    fn peak_abs(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()))
    }

    fn note_on(engine: &mut Engine, note: u8, velocity: u8) {
        engine.apply(ControlEvent::NoteOn { note, velocity });
    }

    fn note_off(engine: &mut Engine, note: u8) {
        engine.apply(ControlEvent::NoteOff { note });
    }

    fn set_wave(engine: &mut Engine, waveform: Waveform) {
        engine.apply(ControlEvent::SetWave { waveform });
    }

    fn set_pulse(engine: &mut Engine, width: f32) {
        engine.apply(ControlEvent::SetPulse { width });
    }

    fn set_cutoff(engine: &mut Engine, hz: f32) {
        engine.apply(ControlEvent::SetCutoff { hz });
    }

    fn set_res(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetResonance { amount });
    }

    fn set_envelope(engine: &mut Engine, which: EnvelopeId, times: AdsrTimes) {
        engine.apply(ControlEvent::SetEnvelope { which, times });
    }

    fn patch_field(engine: &mut Engine, which: EnvelopeId, field: EnvelopeField, value: f32) {
        engine.apply(ControlEvent::PatchEnvelope {
            which,
            field,
            value,
        });
    }

    fn set_fast_amp_sustain(engine: &mut Engine) {
        let mut times = engine.params().amp_env;
        times.attack_ms = 1.0;
        times.decay_ms = 1.0;
        times.sustain = 1.0;
        set_envelope(engine, EnvelopeId::Amp, times);
    }

    fn set_fast_filter_env_sustain(engine: &mut Engine) {
        let mut times = engine.params().filter_env;
        times.attack_ms = 1.0;
        times.decay_ms = 1.0;
        times.sustain = 1.0;
        set_envelope(engine, EnvelopeId::Filter, times);
    }

    fn set_fast_assignable_env_sustain(engine: &mut Engine) {
        let mut times = engine.params().assignable_env;
        times.attack_ms = 1.0;
        times.decay_ms = 1.0;
        times.sustain = 1.0;
        set_envelope(engine, EnvelopeId::Assignable, times);
    }

    fn set_sub_vol(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetSubVol { amount });
    }

    fn set_sub_oct(engine: &mut Engine, octaves: SubOctaves) {
        engine.apply(ControlEvent::SetSubOct { octaves });
    }

    fn set_saw_vol(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetSawVol { amount });
    }

    fn set_square_vol(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetSquareVol { amount });
    }

    fn set_triangle_vol(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetTriangleVol { amount });
    }

    fn set_sine_vol(engine: &mut Engine, amount: f32) {
        engine.apply(ControlEvent::SetSineVol { amount });
    }

    fn set_lfo_dest(engine: &mut Engine, which: LfoId, dest: AssignableDest) {
        engine.apply(ControlEvent::SetLfoDest { which, dest });
    }

    fn set_lfo_amount(engine: &mut Engine, which: LfoId, amount: f32) {
        engine.apply(ControlEvent::SetLfoAmount { which, amount });
    }

    fn set_lfo_rate(engine: &mut Engine, which: LfoId, rate_hz: f32) {
        engine.apply(ControlEvent::SetLfoRate { which, rate_hz });
    }

    fn set_lfo_wave(engine: &mut Engine, which: LfoId, wave: LfoWave) {
        engine.apply(ControlEvent::SetLfoWave { which, wave });
    }

    fn set_lfo_retrig(engine: &mut Engine, which: LfoId, on: bool) {
        engine.apply(ControlEvent::SetLfoRetrig { which, on });
    }

    fn set_square_lfo_one(engine: &mut Engine, dest: AssignableDest, amount: f32) {
        set_lfo_dest(engine, LfoId::One, dest);
        set_lfo_amount(engine, LfoId::One, amount);
        set_lfo_rate(engine, LfoId::One, 1.0);
        set_lfo_wave(engine, LfoId::One, LfoWave::Square);
        set_lfo_retrig(engine, LfoId::One, true);
    }

    #[test]
    fn midi_note_69_is_a4() {
        let hz = midi_note_to_hz(69);
        assert!((hz - 440.0).abs() < 0.01, "expected 440 Hz, got {hz}");
    }

    #[test]
    fn midi_note_60_is_near_middle_c() {
        let hz = midi_note_to_hz(60);
        assert!((hz - 261.63).abs() < 0.1, "expected ~261.63 Hz, got {hz}");
    }

    #[test]
    fn oscillator_stays_within_unit_range() {
        let mut osc = Oscillator::new(SAMPLE_RATE_HZ, 440.0, Waveform::Saw);
        for _ in 0..10_000 {
            let sample = osc.next_sample();
            assert!(
                (-1.5..=1.5).contains(&sample),
                "sample {sample} escaped a safe range"
            );
        }
        for waveform in [Waveform::Square, Waveform::Triangle, Waveform::Sine] {
            osc.set_waveform(waveform);
            osc.set_pulse_width(0.15);
            for _ in 0..10_000 {
                let sample = osc.next_sample();
                assert!(
                    (-1.5..=1.5).contains(&sample),
                    "sample {sample} escaped a safe range for {waveform:?}"
                );
            }
        }
    }

    #[test]
    fn pulse_width_clamps_to_safe_range() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_pulse(&mut engine, 0.0);
        assert!((engine.params().pulse_width - PULSE_WIDTH_MIN).abs() < f32::EPSILON);
        set_pulse(&mut engine, 1.0);
        assert!((engine.params().pulse_width - PULSE_WIDTH_MAX).abs() < f32::EPSILON);
        set_pulse(&mut engine, 0.25);
        assert!((engine.params().pulse_width - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn pulse_width_changes_harmonic_energy() {
        let mut narrow = Engine::new(SAMPLE_RATE_HZ);
        let mut wide = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut narrow, &mut wide] {
            set_wave(engine, Waveform::Square);
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_pulse(&mut narrow, 0.1);
        set_pulse(&mut wide, 0.9);
        note_on(&mut narrow, 48, 127);
        note_on(&mut wide, 48, 127);
        for _ in 0..2_000 {
            narrow.next_sample();
            wide.next_sample();
        }
        let narrow_samples = take_samples(&mut narrow, ANALYSIS_SAMPLES);
        let wide_samples = take_samples(&mut wide, ANALYSIS_SAMPLES);
        // Asymmetric pulses emphasize even harmonics; 50% square would cancel them.
        // Compare 2nd harmonic energy — narrow and wide (mirrored duties) should both
        // be strong and similar, while differing from a 50% square.
        let mut fifty = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut fifty, Waveform::Square);
        set_pulse(&mut fifty, 0.5);
        set_fast_amp_sustain(&mut fifty);
        set_cutoff(&mut fifty, 10_000.0);
        set_res(&mut fifty, 0.0);
        note_on(&mut fifty, 48, 127);
        for _ in 0..2_000 {
            fifty.next_sample();
        }
        let fifty_samples = take_samples(&mut fifty, ANALYSIS_SAMPLES);
        let h2 = midi_note_to_hz(48) * 2.0;
        let narrow_h2 = tone_strength(&narrow_samples, h2);
        let wide_h2 = tone_strength(&wide_samples, h2);
        let fifty_h2 = tone_strength(&fifty_samples, h2);
        assert!(
            narrow_h2 > fifty_h2 * 2.0,
            "narrow pulse should have more 2nd harmonic than 50% square; narrow={narrow_h2} fifty={fifty_h2}"
        );
        assert!(
            wide_h2 > fifty_h2 * 2.0,
            "wide pulse should have more 2nd harmonic than 50% square; wide={wide_h2} fifty={fifty_h2}"
        );
    }

    #[test]
    fn triangle_has_less_high_harmonic_energy_than_saw() {
        let mut saw = Engine::new(SAMPLE_RATE_HZ);
        let mut triangle = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut saw, &mut triangle] {
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_wave(&mut saw, Waveform::Saw);
        set_wave(&mut triangle, Waveform::Triangle);
        note_on(&mut saw, 48, 127);
        note_on(&mut triangle, 48, 127);
        for _ in 0..2_000 {
            saw.next_sample();
            triangle.next_sample();
        }
        let saw_samples = take_samples(&mut saw, ANALYSIS_SAMPLES);
        let tri_samples = take_samples(&mut triangle, ANALYSIS_SAMPLES);
        let h5 = midi_note_to_hz(48) * 5.0;
        let saw_h5 = tone_strength(&saw_samples, h5);
        let tri_h5 = tone_strength(&tri_samples, h5);
        assert!(
            tri_h5 < saw_h5 * 0.5,
            "triangle should be softer at the 5th harmonic than saw; saw={saw_h5} tri={tri_h5}"
        );
    }

    #[test]
    fn sine_concentrates_energy_at_fundamental() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Sine);
        set_fast_amp_sustain(&mut engine);
        set_cutoff(&mut engine, 12_000.0);
        set_res(&mut engine, 0.0);
        let note = 57u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let fund = midi_note_to_hz(note);
        let h5 = fund * 5.0;
        let fund_strength = tone_strength(&samples, fund);
        let h5_strength = tone_strength(&samples, h5);
        assert!(
            fund_strength > TONE_PRESENT,
            "sine should have clear fundamental; strength={fund_strength}"
        );
        assert!(
            h5_strength < fund_strength * 0.1,
            "sine should have little 5th harmonic; fund={fund_strength} h5={h5_strength}"
        );
    }

    #[test]
    fn higher_cutoff_passes_more_high_harmonic_energy() {
        // Square wave at 220 Hz; measure energy near the 5th harmonic (~1100 Hz).
        let mut dark = Engine::new(SAMPLE_RATE_HZ);
        let mut bright = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut dark, Waveform::Square);
        set_wave(&mut bright, Waveform::Square);
        set_cutoff(&mut dark, 400.0);
        set_cutoff(&mut bright, 8_000.0);
        set_res(&mut dark, 0.0);
        set_res(&mut bright, 0.0);
        set_fast_amp_sustain(&mut dark);
        set_fast_amp_sustain(&mut bright);

        note_on(&mut dark, 57, 127); // A3 ≈ 220 Hz
        note_on(&mut bright, 57, 127);
        // Skip attack
        for _ in 0..2_000 {
            dark.next_sample();
            bright.next_sample();
        }
        let dark_samples = take_samples(&mut dark, ANALYSIS_SAMPLES);
        let bright_samples = take_samples(&mut bright, ANALYSIS_SAMPLES);
        let harmonic_hz = midi_note_to_hz(57) * 5.0;
        let dark_h = tone_strength(&dark_samples, harmonic_hz);
        let bright_h = tone_strength(&bright_samples, harmonic_hz);
        assert!(
            bright_h > dark_h * 1.5,
            "expected open cutoff to pass more 5th harmonic; dark={dark_h} bright={bright_h}"
        );
    }

    #[test]
    fn note_off_fades_instead_of_hard_cut() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_envelope(
            &mut engine,
            EnvelopeId::Amp,
            AdsrTimes {
                attack_ms: 1.0,
                decay_ms: 1.0,
                sustain: 1.0,
                release_ms: 50.0,
            },
        );
        note_on(&mut engine, 60, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        note_off(&mut engine, 60);

        let mut first_release_peak = 0.0f32;
        for _ in 0..100 {
            first_release_peak = first_release_peak.max(engine.next_sample().abs());
        }
        assert!(
            first_release_peak > 0.01,
            "should still be audible early in release, peak={first_release_peak}"
        );

        // Wait out release
        for _ in 0..20_000 {
            engine.next_sample();
        }
        let mut late_peak = 0.0f32;
        for _ in 0..1_000 {
            late_peak = late_peak.max(engine.next_sample().abs());
        }
        assert!(
            late_peak < 1e-3,
            "expected silence after release, peak={late_peak}"
        );
    }

    #[test]
    fn fifth_note_prefers_stealing_releasing_voice() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_envelope(
            &mut engine,
            EnvelopeId::Amp,
            AdsrTimes {
                attack_ms: 1.0,
                decay_ms: 1.0,
                sustain: 1.0,
                release_ms: 500.0,
            },
        );

        note_on(&mut engine, 60, 127);
        note_on(&mut engine, 62, 127);
        note_on(&mut engine, 64, 127);
        note_on(&mut engine, 65, 127);
        // Put the oldest note into release so it should be stolen first.
        note_off(&mut engine, 60);
        for _ in 0..100 {
            engine.next_sample();
        }
        note_on(&mut engine, 67, 127);

        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        assert!(
            tone_strength(&samples, midi_note_to_hz(60)) < TONE_ABSENT,
            "releasing note 60 should be stolen"
        );
        for note in [62u8, 64, 65, 67] {
            let strength = tone_strength(&samples, midi_note_to_hz(note));
            assert!(
                strength > TONE_PRESENT,
                "expected note {note} still sounding, strength was {strength}"
            );
        }
    }

    #[test]
    fn velocity_affects_loudness_with_curve() {
        let mut quiet = Engine::new(SAMPLE_RATE_HZ);
        let mut loud = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut quiet, &mut loud] {
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 8_000.0);
        }
        note_on(&mut quiet, 60, 40);
        note_on(&mut loud, 60, 127);

        for _ in 0..2_000 {
            quiet.next_sample();
            loud.next_sample();
        }
        let quiet_samples = take_samples(&mut quiet, ANALYSIS_SAMPLES);
        let loud_samples = take_samples(&mut loud, ANALYSIS_SAMPLES);
        let peak_quiet = peak_abs(&quiet_samples);
        let peak_loud = peak_abs(&loud_samples);
        assert!(
            peak_loud > peak_quiet * 2.0,
            "loud should be much louder with square velocity curve; quiet={peak_quiet} loud={peak_loud}"
        );

        // Ratio should track velocity_amp ratio, not linear velocity ratio.
        let expected_ratio = velocity_to_amp(127) / velocity_to_amp(40);
        let measured_ratio = peak_loud / peak_quiet;
        assert!(
            (measured_ratio - expected_ratio).abs() / expected_ratio < 0.25,
            "peak ratio {measured_ratio} should be near amp ratio {expected_ratio}"
        );
    }

    #[test]
    fn waveform_switch_changes_timbre_energy() {
        let mut saw = Engine::new(SAMPLE_RATE_HZ);
        let mut square = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut saw, &mut square] {
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_wave(&mut saw, Waveform::Saw);
        set_wave(&mut square, Waveform::Square);
        note_on(&mut saw, 48, 127);
        note_on(&mut square, 48, 127);
        for _ in 0..2_000 {
            saw.next_sample();
            square.next_sample();
        }
        let saw_samples = take_samples(&mut saw, ANALYSIS_SAMPLES);
        let square_samples = take_samples(&mut square, ANALYSIS_SAMPLES);
        // Square has stronger odd harmonics; compare 3rd harmonic.
        let h3 = midi_note_to_hz(48) * 3.0;
        let saw_h3 = tone_strength(&saw_samples, h3);
        let square_h3 = tone_strength(&square_samples, h3);
        assert!(
            square_h3 > saw_h3,
            "square should have stronger 3rd harmonic than saw; saw={saw_h3} square={square_h3}"
        );
    }

    #[test]
    fn stacked_octaves_match_summed_offset() {
        let base = 2_000.0;
        let successive = hz_times_octaves(hz_times_octaves(base, 2.0), 2.0);
        let summed = hz_times_octaves(base, 2.0 + 2.0);
        let four = hz_times_octaves(base, 4.0);
        assert!((successive - summed).abs() < 1e-3);
        assert!((summed - four).abs() < 1e-3);
    }

    #[test]
    fn filter_envelope_amount_opens_cutoff_at_sustain() {
        let mut closed = Engine::new(SAMPLE_RATE_HZ);
        let mut opened = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut closed, &mut opened] {
            set_wave(engine, Waveform::Square);
            set_cutoff(engine, 400.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
            set_fast_filter_env_sustain(engine);
        }
        closed.apply(ControlEvent::SetFilterEnvAmount { amount: 0.0 });
        opened.apply(ControlEvent::SetFilterEnvAmount { amount: 4.0 });

        note_on(&mut closed, 57, 127);
        note_on(&mut opened, 57, 127);
        for _ in 0..2_000 {
            closed.next_sample();
            opened.next_sample();
        }
        let closed_samples = take_samples(&mut closed, ANALYSIS_SAMPLES);
        let opened_samples = take_samples(&mut opened, ANALYSIS_SAMPLES);
        let harmonic_hz = midi_note_to_hz(57) * 5.0;
        let closed_h = tone_strength(&closed_samples, harmonic_hz);
        let opened_h = tone_strength(&opened_samples, harmonic_hz);
        assert!(
            opened_h > closed_h * 1.5,
            "expected positive filtenvamt to pass more 5th harmonic; closed={closed_h} opened={opened_h}"
        );
    }

    #[test]
    fn cutoff_stacking_matches_equivalent_filter_amount() {
        let mut stacked = Engine::new(SAMPLE_RATE_HZ);
        let mut combined = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut stacked, &mut combined] {
            set_wave(engine, Waveform::Square);
            set_cutoff(engine, 400.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
            set_fast_filter_env_sustain(engine);
            set_fast_assignable_env_sustain(engine);
        }
        stacked.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Cutoff,
        });
        stacked.apply(ControlEvent::SetFilterEnvAmount { amount: 2.0 });
        stacked.apply(ControlEvent::SetAssignableAmount { amount: 2.0 });
        combined.apply(ControlEvent::SetFilterEnvAmount { amount: 4.0 });
        combined.apply(ControlEvent::SetAssignableAmount { amount: 0.0 });

        note_on(&mut stacked, 57, 127);
        note_on(&mut combined, 57, 127);
        for _ in 0..2_000 {
            stacked.next_sample();
            combined.next_sample();
        }
        let stacked_samples = take_samples(&mut stacked, ANALYSIS_SAMPLES);
        let combined_samples = take_samples(&mut combined, ANALYSIS_SAMPLES);
        let harmonic_hz = midi_note_to_hz(57) * 5.0;
        let stacked_h = tone_strength(&stacked_samples, harmonic_hz);
        let combined_h = tone_strength(&combined_samples, harmonic_hz);
        let denom = stacked_h.max(combined_h).max(1e-6);
        assert!(
            (stacked_h - combined_h).abs() / denom < 0.2,
            "stacked 2+2 octaves should match filter amt 4; stacked={stacked_h} combined={combined_h}"
        );
    }

    #[test]
    fn pitch_dest_plus_one_octave_at_sustain() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_fast_assignable_env_sustain(&mut engine);
        engine.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Pitch,
        });
        engine.apply(ControlEvent::SetAssignableAmount { amount: 1.0 });

        let note = 57u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let base_hz = midi_note_to_hz(note);
        let shifted_hz = base_hz * 2.0;
        let at_base = tone_strength(&samples, base_hz);
        let at_octave = tone_strength(&samples, shifted_hz);
        assert!(
            at_octave > TONE_PRESENT,
            "expected energy near +1 octave ({shifted_hz} Hz), got {at_octave}"
        );
        assert!(
            at_octave > at_base,
            "octave should dominate original pitch; base={at_base} octave={at_octave}"
        );
    }

    #[test]
    fn assignable_dest_pulse_width_at_sustain_changes_harmonic_energy() {
        let mut off = Engine::new(SAMPLE_RATE_HZ);
        let mut pwm = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut off, &mut pwm] {
            set_wave(engine, Waveform::Square);
            set_pulse(engine, 0.5);
            set_fast_amp_sustain(engine);
            set_fast_assignable_env_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        off.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Off,
        });
        pwm.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::PulseWidth,
        });
        pwm.apply(ControlEvent::SetAssignableAmount { amount: -0.4 });

        note_on(&mut off, 48, 127);
        note_on(&mut pwm, 48, 127);
        for _ in 0..2_000 {
            off.next_sample();
            pwm.next_sample();
        }
        let off_samples = take_samples(&mut off, ANALYSIS_SAMPLES);
        let pwm_samples = take_samples(&mut pwm, ANALYSIS_SAMPLES);
        let h2 = midi_note_to_hz(48) * 2.0;
        let off_h2 = tone_strength(&off_samples, h2);
        let pwm_h2 = tone_strength(&pwm_samples, h2);
        assert!(
            pwm_h2 > off_h2 * 2.0,
            "dest pw at sustain should raise 2nd harmonic vs dest off; off={off_h2} pwm={pwm_h2}"
        );
    }

    #[test]
    fn assignable_dest_amp_at_sustain_changes_peak() {
        let mut off = Engine::new(SAMPLE_RATE_HZ);
        let mut boosted = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut off, &mut boosted] {
            set_wave(engine, Waveform::Saw);
            set_fast_amp_sustain(engine);
            set_fast_assignable_env_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        off.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Off,
        });
        off.apply(ControlEvent::SetAssignableAmount { amount: 0.0 });
        boosted.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Amp,
        });
        boosted.apply(ControlEvent::SetAssignableAmount { amount: 0.8 });

        note_on(&mut off, 60, 127);
        note_on(&mut boosted, 60, 127);
        for _ in 0..2_000 {
            off.next_sample();
            boosted.next_sample();
        }
        let peak_off = peak_abs(&take_samples(&mut off, ANALYSIS_SAMPLES));
        let peak_boosted = peak_abs(&take_samples(&mut boosted, ANALYSIS_SAMPLES));
        assert!(
            peak_boosted > peak_off * 1.4,
            "dest amp at sustain should raise peak vs dest off; off={peak_off} boosted={peak_boosted}"
        );
    }

    #[test]
    fn dest_off_and_wave_do_not_change_loudness_through_amp_path() {
        let mut dest_off_zero = Engine::new(SAMPLE_RATE_HZ);
        let mut dest_off_large = Engine::new(SAMPLE_RATE_HZ);
        let mut wave_only = Engine::new(SAMPLE_RATE_HZ);
        let mut dest_amp_zero = Engine::new(SAMPLE_RATE_HZ);
        for engine in [
            &mut dest_off_zero,
            &mut dest_off_large,
            &mut wave_only,
            &mut dest_amp_zero,
        ] {
            set_fast_amp_sustain(engine);
            set_fast_assignable_env_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_wave(&mut dest_off_zero, Waveform::Saw);
        dest_off_zero.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Off,
        });
        dest_off_zero.apply(ControlEvent::SetAssignableAmount { amount: 0.0 });

        set_wave(&mut dest_off_large, Waveform::Saw);
        dest_off_large.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Off,
        });
        dest_off_large.apply(ControlEvent::SetAssignableAmount { amount: 8.0 });

        set_wave(&mut wave_only, Waveform::Saw);

        set_wave(&mut dest_amp_zero, Waveform::Saw);
        dest_amp_zero.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Amp,
        });
        dest_amp_zero.apply(ControlEvent::SetAssignableAmount { amount: 0.0 });

        note_on(&mut dest_off_zero, 60, 127);
        note_on(&mut dest_off_large, 60, 127);
        note_on(&mut wave_only, 60, 127);
        note_on(&mut dest_amp_zero, 60, 127);
        for _ in 0..2_000 {
            dest_off_zero.next_sample();
            dest_off_large.next_sample();
            wave_only.next_sample();
            dest_amp_zero.next_sample();
        }
        let peak_zero = peak_abs(&take_samples(&mut dest_off_zero, ANALYSIS_SAMPLES));
        let peak_large = peak_abs(&take_samples(&mut dest_off_large, ANALYSIS_SAMPLES));
        let peak_wave = peak_abs(&take_samples(&mut wave_only, ANALYSIS_SAMPLES));
        let peak_amp_zero = peak_abs(&take_samples(&mut dest_amp_zero, ANALYSIS_SAMPLES));
        let denom = peak_zero.max(1e-6);
        assert!(
            (peak_large - peak_zero).abs() / denom < 0.05,
            "dest off with large amount must not use the amp path; zero={peak_zero} large={peak_large}"
        );
        assert!(
            (peak_wave - peak_zero).abs() / denom < 0.05,
            "wave preset must not change loudness through the amp path; zero={peak_zero} wave={peak_wave}"
        );
        assert!(
            (peak_amp_zero - peak_zero).abs() / denom < 0.05,
            "dest amp amount 0 must match dest off; zero={peak_zero} amp0={peak_amp_zero}"
        );
    }

    #[test]
    fn envcopy_and_envlink_then_unlink_on_extra_times() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_envelope(
            &mut engine,
            EnvelopeId::Amp,
            AdsrTimes {
                attack_ms: 50.0,
                decay_ms: 80.0,
                sustain: 0.4,
                release_ms: 150.0,
            },
        );

        engine.apply(ControlEvent::EnvCopy);
        assert!((engine.params().filter_env.attack_ms - 50.0).abs() < f32::EPSILON);
        assert!((engine.params().filter_env.decay_ms - 80.0).abs() < f32::EPSILON);
        assert!((engine.params().filter_env.sustain - 0.4).abs() < f32::EPSILON);
        assert!((engine.params().filter_env.release_ms - 150.0).abs() < f32::EPSILON);
        assert!((engine.params().assignable_env.attack_ms - 50.0).abs() < f32::EPSILON);
        assert!(!engine.params().env_link);

        engine.apply(ControlEvent::SetEnvLink { on: true });
        assert!(engine.params().env_link);
        patch_field(&mut engine, EnvelopeId::Amp, EnvelopeField::Attack, 90.0);
        assert!((engine.params().filter_env.attack_ms - 90.0).abs() < f32::EPSILON);
        assert!((engine.params().assignable_env.attack_ms - 90.0).abs() < f32::EPSILON);

        patch_field(&mut engine, EnvelopeId::Filter, EnvelopeField::Attack, 12.0);
        assert!(!engine.params().env_link);
        assert!((engine.params().filter_env.attack_ms - 12.0).abs() < f32::EPSILON);
        patch_field(&mut engine, EnvelopeId::Amp, EnvelopeField::Attack, 200.0);
        assert!((engine.params().amp_env.attack_ms - 200.0).abs() < f32::EPSILON);
        assert!((engine.params().filter_env.attack_ms - 12.0).abs() < f32::EPSILON);

        engine.apply(ControlEvent::SetEnvLink { on: true });
        patch_field(
            &mut engine,
            EnvelopeId::Assignable,
            EnvelopeField::Decay,
            33.0,
        );
        assert!(!engine.params().env_link);
        assert!((engine.params().assignable_env.decay_ms - 33.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_envelope_clamps_times_and_sustain() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_envelope(
            &mut engine,
            EnvelopeId::Amp,
            AdsrTimes {
                attack_ms: -5.0,
                decay_ms: -1.0,
                sustain: 2.0,
                release_ms: -8.0,
            },
        );
        let amp = engine.params().amp_env;
        assert_eq!(amp.attack_ms, 0.0);
        assert_eq!(amp.decay_ms, 0.0);
        assert_eq!(amp.sustain, 1.0);
        assert_eq!(amp.release_ms, 0.0);
    }

    #[test]
    fn env_vel_scales_pitch_dest_by_velocity() {
        let mut quiet = Engine::new(SAMPLE_RATE_HZ);
        let mut loud = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut quiet, &mut loud] {
            set_wave(engine, Waveform::Saw);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
            set_fast_assignable_env_sustain(engine);
            engine.apply(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Pitch,
            });
            engine.apply(ControlEvent::SetAssignableAmount { amount: 1.0 });
            engine.apply(ControlEvent::SetEnvVel { amount: 1.0 });
        }

        let note = 57u8;
        note_on(&mut quiet, note, 32);
        note_on(&mut loud, note, 127);
        for _ in 0..2_000 {
            quiet.next_sample();
            loud.next_sample();
        }
        let quiet_samples = take_samples(&mut quiet, ANALYSIS_SAMPLES);
        let loud_samples = take_samples(&mut loud, ANALYSIS_SAMPLES);
        let base_hz = midi_note_to_hz(note);
        let octave_hz = base_hz * 2.0;
        let quiet_at_octave = tone_strength(&quiet_samples, octave_hz);
        let loud_at_octave = tone_strength(&loud_samples, octave_hz);
        let quiet_at_base = tone_strength(&quiet_samples, base_hz);
        assert!(
            loud_at_octave > quiet_at_octave,
            "high velocity should shift pitch more toward +1 octave; quiet={quiet_at_octave} loud={loud_at_octave}"
        );
        assert!(
            quiet_at_base > quiet_at_octave,
            "low velocity with env_vel 1 should stay nearer the original pitch; base={quiet_at_base} octave={quiet_at_octave}"
        );
    }

    #[test]
    fn sub_defaults_silent_and_one_octave() {
        let engine = Engine::new(SAMPLE_RATE_HZ);
        assert!((engine.params().sub_vol - 0.0).abs() < f32::EPSILON);
        assert_eq!(engine.params().sub_octaves, SubOctaves::One);
    }

    #[test]
    fn sub_vol_zero_matches_main_only_spectrum() {
        let mut with_sub_off = Engine::new(SAMPLE_RATE_HZ);
        let mut main_only = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut with_sub_off, &mut main_only] {
            set_wave(engine, Waveform::Saw);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
        }
        set_sub_vol(&mut with_sub_off, 0.0);
        set_sub_oct(&mut with_sub_off, SubOctaves::One);

        let note = 60u8;
        note_on(&mut with_sub_off, note, 127);
        note_on(&mut main_only, note, 127);
        for _ in 0..2_000 {
            with_sub_off.next_sample();
            main_only.next_sample();
        }
        let off_samples = take_samples(&mut with_sub_off, ANALYSIS_SAMPLES);
        let main_samples = take_samples(&mut main_only, ANALYSIS_SAMPLES);
        let fund = midi_note_to_hz(note);
        let sub_hz = fund / 2.0;
        let off_fund = tone_strength(&off_samples, fund);
        let main_fund = tone_strength(&main_samples, fund);
        let off_sub = tone_strength(&off_samples, sub_hz);
        let main_sub = tone_strength(&main_samples, sub_hz);
        let denom = off_fund.max(main_fund).max(1e-6);
        assert!(
            (off_fund - main_fund).abs() / denom < 0.15,
            "subvol 0 should match main-only fundamental; off={off_fund} main={main_fund}"
        );
        assert!(
            (off_sub - main_sub).abs() < TONE_ABSENT,
            "subvol 0 should not add sub energy; off={off_sub} main={main_sub}"
        );
    }

    #[test]
    fn sub_octave_one_adds_energy_at_half_frequency() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_sub_vol(&mut engine, 1.0);
        set_sub_oct(&mut engine, SubOctaves::One);

        let note = 60u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let fund = midi_note_to_hz(note);
        let sub_hz = fund / 2.0;
        let at_fund = tone_strength(&samples, fund);
        let at_sub = tone_strength(&samples, sub_hz);
        assert!(
            at_sub > TONE_PRESENT,
            "expected energy at one octave below ({sub_hz} Hz), got {at_sub}"
        );
        assert!(
            at_fund > TONE_PRESENT,
            "main fundamental should still be present; got {at_fund}"
        );
    }

    #[test]
    fn sub_octave_two_adds_energy_at_quarter_frequency() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_sub_vol(&mut engine, 1.0);
        set_sub_oct(&mut engine, SubOctaves::Two);

        let note = 72u8; // higher so quarter-freq stays well above analysis floor
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let fund = midi_note_to_hz(note);
        let sub_hz = fund / 4.0;
        let at_sub = tone_strength(&samples, sub_hz);
        let at_one_oct = tone_strength(&samples, fund / 2.0);
        assert!(
            at_sub > TONE_PRESENT,
            "expected energy at two octaves below ({sub_hz} Hz), got {at_sub}"
        );
        assert!(
            at_sub > at_one_oct,
            "two-octave sub should dominate one-octave bin; sub={at_sub} one_oct={at_one_oct}"
        );
    }

    #[test]
    fn sub_tracks_assignable_pitch_destination() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_fast_assignable_env_sustain(&mut engine);
        engine.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Pitch,
        });
        engine.apply(ControlEvent::SetAssignableAmount { amount: 1.0 });
        set_sub_vol(&mut engine, 1.0);
        set_sub_oct(&mut engine, SubOctaves::One);

        let note = 57u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let base_hz = midi_note_to_hz(note);
        let shifted_main = base_hz * 2.0;
        let shifted_sub = shifted_main / 2.0; // same as base_hz, but we check relative to unshifted sub
        let unshifted_sub = base_hz / 2.0;
        let at_shifted_sub = tone_strength(&samples, shifted_sub);
        let at_unshifted_sub = tone_strength(&samples, unshifted_sub);
        // With +1 octave pitch env, main is at 2*base and sub (1 oct down) lands at base.
        // Unshifted sub would be at base/2 and should be weaker.
        assert!(
            at_shifted_sub > TONE_PRESENT,
            "sub should follow sounding pitch to {shifted_sub} Hz; got {at_shifted_sub}"
        );
        assert!(
            at_shifted_sub > at_unshifted_sub,
            "sub should track pitch env, not stay at MIDI-only sub; shifted={at_shifted_sub} frozen={at_unshifted_sub}"
        );
    }

    #[test]
    fn default_patch_is_saw_solo() {
        let engine = Engine::new(SAMPLE_RATE_HZ);
        assert!((engine.params().saw_vol - 1.0).abs() < f32::EPSILON);
        assert!((engine.params().square_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().triangle_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().sine_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().sub_vol - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn wave_preset_sets_levels_and_zeros_sub() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_sub_vol(&mut engine, 0.5);
        set_wave(&mut engine, Waveform::Triangle);
        assert!((engine.params().saw_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().square_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().triangle_vol - 1.0).abs() < f32::EPSILON);
        assert!((engine.params().sine_vol - 0.0).abs() < f32::EPSILON);
        assert!((engine.params().sub_vol - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn single_active_osc_level_normalizes_to_full_scale() {
        let mut full = Engine::new(SAMPLE_RATE_HZ);
        let mut partial = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut full, &mut partial] {
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_wave(&mut full, Waveform::Saw);
        set_saw_vol(&mut partial, 0.25);

        note_on(&mut full, 48, 127);
        note_on(&mut partial, 48, 127);
        for _ in 0..2_000 {
            full.next_sample();
            partial.next_sample();
        }
        let full_peak = peak_abs(&take_samples(&mut full, ANALYSIS_SAMPLES));
        let partial_peak = peak_abs(&take_samples(&mut partial, ANALYSIS_SAMPLES));
        let denom = full_peak.max(1e-6);
        assert!(
            (full_peak - partial_peak).abs() / denom < 0.15,
            "solo saw should sound the same at any non-zero level; full={full_peak} partial={partial_peak}"
        );
    }

    #[test]
    fn mixed_saw_square_differs_from_saw_alone() {
        let mut saw_only = Engine::new(SAMPLE_RATE_HZ);
        let mut blend = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut saw_only, &mut blend] {
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_wave(&mut saw_only, Waveform::Saw);
        set_saw_vol(&mut blend, 0.5);
        set_square_vol(&mut blend, 0.5);

        note_on(&mut saw_only, 48, 127);
        note_on(&mut blend, 48, 127);
        for _ in 0..2_000 {
            saw_only.next_sample();
            blend.next_sample();
        }
        let saw_samples = take_samples(&mut saw_only, ANALYSIS_SAMPLES);
        let blend_samples = take_samples(&mut blend, ANALYSIS_SAMPLES);
        let h3 = midi_note_to_hz(48) * 3.0;
        let saw_h3 = tone_strength(&saw_samples, h3);
        let blend_h3 = tone_strength(&blend_samples, h3);
        assert!(
            (saw_h3 - blend_h3).abs() > 1e-4,
            "saw+square blend should differ spectrally from saw alone; saw={saw_h3} blend={blend_h3}"
        );
    }

    #[test]
    fn sub_only_when_all_at_pitch_levels_zero() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_saw_vol(&mut engine, 0.0);
        set_square_vol(&mut engine, 0.0);
        set_triangle_vol(&mut engine, 0.0);
        set_sine_vol(&mut engine, 0.0);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_sub_vol(&mut engine, 1.0);
        set_sub_oct(&mut engine, SubOctaves::One);

        let note = 60u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let fund = midi_note_to_hz(note);
        let sub_hz = fund / 2.0;
        let at_fund = tone_strength(&samples, fund);
        let at_sub = tone_strength(&samples, sub_hz);
        assert!(
            at_sub > TONE_PRESENT,
            "sub-only mix should still sound at sub frequency; got {at_sub}"
        );
        assert!(
            at_fund < TONE_PRESENT,
            "sub-only mix should not have strong fundamental; got {at_fund}"
        );
    }

    #[test]
    fn osc_level_clamps_to_unit_range() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_saw_vol(&mut engine, -0.5);
        assert!((engine.params().saw_vol - 0.0).abs() < f32::EPSILON);
        set_square_vol(&mut engine, 1.5);
        assert!((engine.params().square_vol - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sub_vol_clamps_to_unit_range() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_sub_vol(&mut engine, -0.5);
        assert!((engine.params().sub_vol - 0.0).abs() < f32::EPSILON);
        set_sub_vol(&mut engine, 1.5);
        assert!((engine.params().sub_vol - 1.0).abs() < f32::EPSILON);
        set_sub_vol(&mut engine, 0.4);
        assert!((engine.params().sub_vol - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn param_effects_classify_pulse_and_ordinary_params() {
        let mut params = EngineParams::default();
        let pulse = params
            .apply(ControlEvent::SetPulse { width: 0.25 })
            .expect("parameter event");
        assert!(pulse.synchronize_pulse_width);
        assert!(!pulse.synchronize_envelopes);

        let ordinary = params
            .apply(ControlEvent::SetCutoff { hz: 800.0 })
            .expect("parameter event");
        assert!(!ordinary.synchronize_pulse_width);
        assert!(!ordinary.synchronize_envelopes);
    }

    #[test]
    fn param_effects_classify_every_envelope_update() {
        let mut params = EngineParams::default();
        let events = [
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Amp,
                times: AdsrTimes::default(),
            },
            ControlEvent::PatchEnvelope {
                which: EnvelopeId::Filter,
                field: EnvelopeField::Attack,
                value: 10.0,
            },
            ControlEvent::EnvCopy,
            ControlEvent::SetEnvLink { on: true },
        ];

        for event in events {
            let effects = params.apply(event).expect("parameter event");
            assert!(!effects.synchronize_pulse_width);
            assert!(effects.synchronize_envelopes, "event was {event:?}");
        }

        let unlink = params
            .apply(ControlEvent::SetEnvLink { on: false })
            .expect("parameter event");
        assert!(!unlink.synchronize_pulse_width);
        assert!(!unlink.synchronize_envelopes);
    }

    #[test]
    fn note_events_report_no_param_effects() {
        let mut params = EngineParams::default();
        assert_eq!(
            params.apply(ControlEvent::NoteOn {
                note: 60,
                velocity: 100,
            }),
            None
        );
        assert_eq!(params.apply(ControlEvent::NoteOff { note: 60 }), None);
    }

    #[test]
    fn lfo_defaults_and_rate_amount_clamp() {
        let params = EngineParams::default();
        for lfo in params.lfos {
            assert_eq!(lfo.dest, AssignableDest::Off);
            assert!((lfo.amount - 0.0).abs() < f32::EPSILON);
            assert!((lfo.rate_hz - LFO_RATE_DEFAULT_HZ).abs() < f32::EPSILON);
            assert_eq!(lfo.wave, LfoWave::Sine);
            assert!(lfo.retrigger);
        }

        let mut params = EngineParams::default();
        params
            .apply(ControlEvent::SetLfoRate {
                which: LfoId::One,
                rate_hz: 0.0,
            })
            .expect("parameter event");
        assert!((params.lfos[0].rate_hz - LFO_RATE_MIN_HZ).abs() < f32::EPSILON);
        params
            .apply(ControlEvent::SetLfoRate {
                which: LfoId::Two,
                rate_hz: 100.0,
            })
            .expect("parameter event");
        assert!((params.lfos[1].rate_hz - LFO_RATE_MAX_HZ).abs() < f32::EPSILON);
        params
            .apply(ControlEvent::SetLfoAmount {
                which: LfoId::One,
                amount: -20.0,
            })
            .expect("parameter event");
        assert!((params.lfos[0].amount - AMT_MIN).abs() < f32::EPSILON);
        params
            .apply(ControlEvent::SetLfoAmount {
                which: LfoId::One,
                amount: 20.0,
            })
            .expect("parameter event");
        assert!((params.lfos[0].amount - AMT_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn square_lfo_pitch_sits_at_plus_one_octave() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_square_lfo_one(&mut engine, AssignableDest::Pitch, 1.0);

        let note = 57u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let base_hz = midi_note_to_hz(note);
        let shifted_hz = base_hz * 2.0;
        let at_base = tone_strength(&samples, base_hz);
        let at_octave = tone_strength(&samples, shifted_hz);
        assert!(
            at_octave > TONE_PRESENT,
            "square LFO dest pitch amt 1 should sit near +1 octave ({shifted_hz} Hz), got {at_octave}"
        );
        assert!(
            at_octave > at_base,
            "octave should dominate original pitch; base={at_base} octave={at_octave}"
        );
    }

    #[test]
    fn square_lfo_cutoff_opens_vs_amount_zero() {
        let mut closed = Engine::new(SAMPLE_RATE_HZ);
        let mut opened = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut closed, &mut opened] {
            set_wave(engine, Waveform::Square);
            set_cutoff(engine, 400.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
        }
        set_square_lfo_one(&mut closed, AssignableDest::Cutoff, 0.0);
        set_square_lfo_one(&mut opened, AssignableDest::Cutoff, 4.0);

        note_on(&mut closed, 57, 127);
        note_on(&mut opened, 57, 127);
        for _ in 0..2_000 {
            closed.next_sample();
            opened.next_sample();
        }
        let closed_samples = take_samples(&mut closed, ANALYSIS_SAMPLES);
        let opened_samples = take_samples(&mut opened, ANALYSIS_SAMPLES);
        let harmonic_hz = midi_note_to_hz(57) * 5.0;
        let closed_h = tone_strength(&closed_samples, harmonic_hz);
        let opened_h = tone_strength(&opened_samples, harmonic_hz);
        assert!(
            opened_h > closed_h * 1.5,
            "square LFO dest cutoff should pass more 5th harmonic; closed={closed_h} opened={opened_h}"
        );
    }

    #[test]
    fn lfo_dest_off_with_large_amount_does_not_shift_pitch() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_square_lfo_one(&mut engine, AssignableDest::Off, 8.0);

        let note = 57u8;
        note_on(&mut engine, note, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let base_hz = midi_note_to_hz(note);
        let octave_hz = base_hz * 2.0;
        let at_base = tone_strength(&samples, base_hz);
        let at_octave = tone_strength(&samples, octave_hz);
        assert!(
            at_base > TONE_PRESENT,
            "dest off must keep the original pitch; got {at_base}"
        );
        assert!(
            at_base > at_octave,
            "dest off with large amount must not shift an octave; base={at_base} octave={at_octave}"
        );
    }

    #[test]
    fn square_lfo_dest_amp_changes_peak() {
        let mut off = Engine::new(SAMPLE_RATE_HZ);
        let mut boosted = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut off, &mut boosted] {
            set_wave(engine, Waveform::Saw);
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_square_lfo_one(&mut off, AssignableDest::Off, 0.0);
        set_square_lfo_one(&mut boosted, AssignableDest::Amp, 0.8);

        note_on(&mut off, 60, 127);
        note_on(&mut boosted, 60, 127);
        for _ in 0..2_000 {
            off.next_sample();
            boosted.next_sample();
        }
        let peak_off = peak_abs(&take_samples(&mut off, ANALYSIS_SAMPLES));
        let peak_boosted = peak_abs(&take_samples(&mut boosted, ANALYSIS_SAMPLES));
        assert!(
            peak_boosted > peak_off * 1.4,
            "square LFO dest amp should raise peak vs dest off; off={peak_off} boosted={peak_boosted}"
        );
    }

    #[test]
    fn square_lfo_dest_pulse_width_changes_harmonic_energy() {
        let mut off = Engine::new(SAMPLE_RATE_HZ);
        let mut pwm = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut off, &mut pwm] {
            set_wave(engine, Waveform::Square);
            set_pulse(engine, 0.5);
            set_fast_amp_sustain(engine);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
        }
        set_square_lfo_one(&mut off, AssignableDest::Off, 0.0);
        set_square_lfo_one(&mut pwm, AssignableDest::PulseWidth, -0.4);

        note_on(&mut off, 48, 127);
        note_on(&mut pwm, 48, 127);
        for _ in 0..2_000 {
            off.next_sample();
            pwm.next_sample();
        }
        let off_samples = take_samples(&mut off, ANALYSIS_SAMPLES);
        let pwm_samples = take_samples(&mut pwm, ANALYSIS_SAMPLES);
        let h2 = midi_note_to_hz(48) * 2.0;
        let off_h2 = tone_strength(&off_samples, h2);
        let pwm_h2 = tone_strength(&pwm_samples, h2);
        assert!(
            pwm_h2 > off_h2 * 2.0,
            "square LFO dest pw should raise 2nd harmonic vs dest off; off={off_h2} pwm={pwm_h2}"
        );
    }

    #[test]
    fn assignable_env_plus_lfo_cutoff_matches_summed_amount() {
        let mut stacked = Engine::new(SAMPLE_RATE_HZ);
        let mut combined = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut stacked, &mut combined] {
            set_wave(engine, Waveform::Square);
            set_cutoff(engine, 400.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
            set_fast_assignable_env_sustain(engine);
        }
        stacked.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Cutoff,
        });
        stacked.apply(ControlEvent::SetAssignableAmount { amount: 2.0 });
        set_square_lfo_one(&mut stacked, AssignableDest::Cutoff, 2.0);

        combined.apply(ControlEvent::SetAssignableDest {
            dest: AssignableDest::Cutoff,
        });
        combined.apply(ControlEvent::SetAssignableAmount { amount: 4.0 });
        set_square_lfo_one(&mut combined, AssignableDest::Off, 0.0);

        note_on(&mut stacked, 57, 127);
        note_on(&mut combined, 57, 127);
        for _ in 0..2_000 {
            stacked.next_sample();
            combined.next_sample();
        }
        let stacked_samples = take_samples(&mut stacked, ANALYSIS_SAMPLES);
        let combined_samples = take_samples(&mut combined, ANALYSIS_SAMPLES);
        let harmonic_hz = midi_note_to_hz(57) * 5.0;
        let stacked_h = tone_strength(&stacked_samples, harmonic_hz);
        let combined_h = tone_strength(&combined_samples, harmonic_hz);
        let denom = stacked_h.max(combined_h).max(1e-6);
        assert!(
            (stacked_h - combined_h).abs() / denom < 0.2,
            "asenv 2 + LFO 2 octaves should match asenv 4; stacked={stacked_h} combined={combined_h}"
        );
    }

    #[test]
    fn square_lfo_pitch_retrig_starts_each_note_at_plus_octave() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        set_wave(&mut engine, Waveform::Saw);
        set_cutoff(&mut engine, 10_000.0);
        set_res(&mut engine, 0.0);
        set_fast_amp_sustain(&mut engine);
        set_square_lfo_one(&mut engine, AssignableDest::Pitch, 1.0);

        let note = 57u8;
        let base_hz = midi_note_to_hz(note);
        let octave_hz = base_hz * 2.0;

        note_on(&mut engine, note, 127);
        for _ in 0..500 {
            engine.next_sample();
        }
        let first = take_samples(&mut engine, 1_024);
        note_off(&mut engine, note);
        for _ in 0..20_000 {
            engine.next_sample();
        }
        note_on(&mut engine, note, 127);
        for _ in 0..500 {
            engine.next_sample();
        }
        let second = take_samples(&mut engine, 1_024);

        for (label, samples) in [("first", first.as_slice()), ("second", second.as_slice())] {
            let at_octave = tone_strength(samples, octave_hz);
            let at_base = tone_strength(samples, base_hz);
            assert!(
                at_octave > TONE_PRESENT,
                "{label} note should start at +1 octave; got {at_octave}"
            );
            assert!(
                at_octave > at_base,
                "{label} note octave should dominate; base={at_base} octave={at_octave}"
            );
        }
    }
}
