//! Fixed voice ownership, allocation, and subtractive sample rendering.

use crate::envelope::velocity_to_amp;
use crate::filter::Svf;
use crate::oscillator::{Oscillator, Waveform};
use crate::{AssignableDest, EngineParams, VOICE_COUNT, hz_times_octaves, midi_note_to_hz};

/// Conservative per-voice gain so a few bright voices stay near full scale.
const VOICE_AMPLITUDE: f32 = 0.12;

#[derive(Clone, Copy)]
struct AtPitchLevels {
    saw: f32,
    square: f32,
    triangle: f32,
    sine: f32,
}

struct Voice {
    saw: Oscillator,
    square: Oscillator,
    triangle: Oscillator,
    sine: Oscillator,
    sub: Oscillator,
    filter: Svf,
    amp: crate::Adsr,
    filter_env: crate::Adsr,
    assignable_env: crate::Adsr,
    note: u8,
    velocity_amp: f32,
    base_hz: f32,
    /// Monotonic age stamp; higher means more recently started (used for steal).
    age: u32,
}

impl Voice {
    fn new(sample_rate_hz: f32) -> Self {
        Self {
            saw: Oscillator::new(sample_rate_hz, 440.0, Waveform::Saw),
            square: Oscillator::new(sample_rate_hz, 440.0, Waveform::Square),
            triangle: Oscillator::new(sample_rate_hz, 440.0, Waveform::Triangle),
            sine: Oscillator::new(sample_rate_hz, 440.0, Waveform::Sine),
            sub: Oscillator::new(sample_rate_hz, 220.0, Waveform::Sine),
            filter: Svf::new(),
            amp: crate::Adsr::new(sample_rate_hz),
            filter_env: crate::Adsr::new(sample_rate_hz),
            assignable_env: crate::Adsr::new(sample_rate_hz),
            note: 0,
            velocity_amp: 1.0,
            base_hz: 440.0,
            age: 0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    fn is_releasing(&self) -> bool {
        self.amp.is_releasing()
    }

    fn synchronize_envelopes(&mut self, params: &EngineParams) {
        params.amp_env.apply_to(&mut self.amp);
        params.filter_env.apply_to(&mut self.filter_env);
        params.assignable_env.apply_to(&mut self.assignable_env);
    }

    fn start(
        &mut self,
        sample_rate_hz: f32,
        note: u8,
        velocity: u8,
        age: u32,
        params: &EngineParams,
    ) {
        let base_hz = midi_note_to_hz(note);
        self.saw = Oscillator::new(sample_rate_hz, base_hz, Waveform::Saw);
        self.square = Oscillator::new(sample_rate_hz, base_hz, Waveform::Square);
        self.square.set_pulse_width(params.pulse_width);
        self.triangle = Oscillator::new(sample_rate_hz, base_hz, Waveform::Triangle);
        self.sine = Oscillator::new(sample_rate_hz, base_hz, Waveform::Sine);
        let sub_hz =
            (base_hz / params.sub_octaves.frequency_divisor()).clamp(20.0, sample_rate_hz * 0.25);
        self.sub = Oscillator::new(sample_rate_hz, sub_hz, Waveform::Sine);
        self.filter.reset();
        self.synchronize_envelopes(params);
        self.amp.note_on();
        self.filter_env.note_on();
        self.assignable_env.note_on();
        self.note = note;
        self.velocity_amp = velocity_to_amp(velocity);
        self.base_hz = base_hz;
        self.age = age;
    }

    fn release(&mut self) {
        self.amp.note_off();
        self.filter_env.note_off();
        self.assignable_env.note_off();
    }

    fn silence(&mut self) {
        self.amp.force_idle();
        self.filter_env.force_idle();
        self.assignable_env.force_idle();
    }

    fn render_sample(&mut self, sample_rate_hz: f32, params: &EngineParams) -> f32 {
        if !self.is_active() {
            return 0.0;
        }

        let filter_level = self.filter_env.next_level();
        let assign_level = self.assignable_env.next_level();
        let velocity = self.velocity_amp;
        let filter_octaves = filter_level
            * effective_envelope_amount(params.filter_env_amount, params.env_vel, velocity);
        let assign_amount =
            effective_envelope_amount(params.assignable_amount, params.env_vel, velocity);

        let (assign_cutoff_octaves, resonance, oscillator_hz) = match params.assignable_dest {
            AssignableDest::Off => (0.0, params.resonance, self.base_hz),
            AssignableDest::Resonance => (
                0.0,
                params.resonance + assign_level * assign_amount,
                self.base_hz,
            ),
            AssignableDest::Pitch => {
                let hz = hz_times_octaves(self.base_hz, assign_level * assign_amount);
                let hz = hz.clamp(20.0, sample_rate_hz * 0.25);
                (0.0, params.resonance, hz)
            }
            AssignableDest::Cutoff => {
                (assign_level * assign_amount, params.resonance, self.base_hz)
            }
        };
        let cutoff_hz = hz_times_octaves(params.cutoff_hz, filter_octaves + assign_cutoff_octaves);
        let levels = AtPitchLevels {
            saw: params.saw_vol,
            square: params.square_vol,
            triangle: params.triangle_vol,
            sine: params.sine_vol,
        };

        if levels.saw > 0.0 {
            self.saw.set_frequency(sample_rate_hz, oscillator_hz);
        }
        if levels.square > 0.0 {
            self.square.set_frequency(sample_rate_hz, oscillator_hz);
        }
        if levels.triangle > 0.0 {
            self.triangle.set_frequency(sample_rate_hz, oscillator_hz);
        }
        if levels.sine > 0.0 {
            self.sine.set_frequency(sample_rate_hz, oscillator_hz);
        }

        let samples = [
            if levels.saw > 0.0 {
                self.saw.next_sample()
            } else {
                0.0
            },
            if levels.square > 0.0 {
                self.square.next_sample()
            } else {
                0.0
            },
            if levels.triangle > 0.0 {
                self.triangle.next_sample()
            } else {
                0.0
            },
            if levels.sine > 0.0 {
                self.sine.next_sample()
            } else {
                0.0
            },
        ];
        let main = normalize_blend(levels, samples);
        let oscillator = if params.sub_vol > 0.0 {
            let sub_hz = (oscillator_hz / params.sub_octaves.frequency_divisor())
                .clamp(20.0, sample_rate_hz * 0.25);
            self.sub.set_frequency(sample_rate_hz, sub_hz);
            main + params.sub_vol * self.sub.next_sample()
        } else {
            main
        };
        let filtered = self
            .filter
            .process(oscillator, sample_rate_hz, cutoff_hz, resonance);
        let amp = self.amp.next_level();
        filtered * amp * self.velocity_amp * VOICE_AMPLITUDE
    }
}

pub(crate) struct Voices {
    sample_rate_hz: f32,
    voices: [Voice; VOICE_COUNT],
    next_age: u32,
}

impl Voices {
    pub(crate) fn new(sample_rate_hz: f32, params: &EngineParams) -> Self {
        let mut voices = Self {
            sample_rate_hz,
            voices: core::array::from_fn(|_| Voice::new(sample_rate_hz)),
            next_age: 1,
        };
        voices.synchronize_envelopes(params);
        voices
    }

    pub(crate) fn note_on(&mut self, note: u8, velocity: u8, params: &EngineParams) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }

        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        let index = self
            .voices
            .iter()
            .position(|voice| voice.is_active() && voice.note == note)
            .or_else(|| self.voices.iter().position(|voice| !voice.is_active()))
            .unwrap_or_else(|| self.steal_index());
        self.voices[index].start(self.sample_rate_hz, note, velocity, age, params);
    }

    pub(crate) fn note_off(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.is_active() && voice.note == note {
                voice.release();
            }
        }
    }

    pub(crate) fn silence(&mut self) {
        for voice in &mut self.voices {
            voice.silence();
        }
    }

    pub(crate) fn synchronize_envelopes(&mut self, params: &EngineParams) {
        for voice in &mut self.voices {
            voice.synchronize_envelopes(params);
        }
    }

    pub(crate) fn synchronize_pulse_width(&mut self, width: f32) {
        for voice in &mut self.voices {
            voice.square.set_pulse_width(width);
        }
    }

    pub(crate) fn render_sample(&mut self, params: &EngineParams) -> f32 {
        self.voices
            .iter_mut()
            .map(|voice| voice.render_sample(self.sample_rate_hz, params))
            .sum()
    }

    fn steal_index(&self) -> usize {
        self.voices
            .iter()
            .enumerate()
            .filter(|(_, voice)| voice.is_releasing())
            .min_by_key(|(_, voice)| voice.age)
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, voice)| voice.age)
            })
            .map(|(index, _)| index)
            .expect("VOICE_COUNT is non-zero")
    }
}

fn effective_envelope_amount(amount: f32, env_vel: f32, velocity: f32) -> f32 {
    amount * (1.0 - env_vel + env_vel * velocity)
}

fn normalize_blend(levels: AtPitchLevels, samples: [f32; 4]) -> f32 {
    let sum = levels.saw + levels.square + levels.triangle + levels.sine;
    if sum == 0.0 {
        return 0.0;
    }
    (levels.saw * samples[0]
        + levels.square * samples[1]
        + levels.triangle * samples[2]
        + levels.sine * samples[3])
        / sum
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;

    #[test]
    fn fifth_note_steals_oldest_releasing_voice() {
        let params = EngineParams::default();
        let mut voices = Voices::new(SAMPLE_RATE_HZ, &params);
        for note in [60, 62, 64, 65] {
            voices.note_on(note, 127, &params);
        }
        voices.note_off(60);
        voices.note_on(67, 127, &params);

        let active_notes: [u8; VOICE_COUNT] =
            core::array::from_fn(|index| voices.voices[index].note);
        assert!(!active_notes.contains(&60));
        for note in [62, 64, 65, 67] {
            assert!(active_notes.contains(&note));
        }
    }

    #[test]
    fn four_notes_keep_independent_voice_state() {
        let params = EngineParams::default();
        let mut voices = Voices::new(SAMPLE_RATE_HZ, &params);
        for note in [60, 62, 64, 65] {
            voices.note_on(note, 127, &params);
        }

        let notes = core::array::from_fn(|index| voices.voices[index].note);
        assert_eq!(notes, [60, 62, 64, 65]);
        assert!(voices.render_sample(&params).is_finite());
    }
}
