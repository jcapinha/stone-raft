//! Fixed voice ownership, allocation, and subtractive sample rendering.
//!
//! Pitch, cutoff, resonance, pulse width, filter and assignable envelopes, and
//! LFO levels are refreshed once per 32-sample block. That matches the Daisy
//! callback. Waveform generation and the amplitude envelope still run at 48 kHz.

use crate::envelope::velocity_to_amp;
use crate::filter::Svf;
use crate::lfo::Lfo;
use crate::oscillator::{Oscillator, PULSE_WIDTH_MAX, PULSE_WIDTH_MIN, Waveform};
use crate::{AssignableDest, EngineParams, VOICE_COUNT, hz_times_octaves, midi_note_to_hz};

#[derive(Clone, Copy, Default)]
struct ModOffsets {
    cutoff_octaves: f32,
    resonance: f32,
    pitch_octaves: f32,
    pulse_width: f32,
    amp: f32,
}

/// Adds `level * amount` to one dest. Envelope and LFO sources both use this.
fn add_assignable(offsets: &mut ModOffsets, dest: AssignableDest, level: f32, amount: f32) {
    let delta = level * amount;
    match dest {
        AssignableDest::Off => {}
        AssignableDest::Resonance => offsets.resonance += delta,
        AssignableDest::Pitch => offsets.pitch_octaves += delta,
        AssignableDest::Cutoff => offsets.cutoff_octaves += delta,
        AssignableDest::PulseWidth => offsets.pulse_width += delta,
        AssignableDest::Amp => offsets.amp += delta,
    }
}

/// Conservative per-voice gain so a few bright voices stay near full scale.
const VOICE_AMPLITUDE: f32 = 0.12;
/// Slow controls stay fixed for one Daisy callback, then move again.
const CONTROL_BLOCK_SAMPLES: usize = 32;

#[derive(Default)]
struct HeldControl {
    saw_gain: f32,
    square_gain: f32,
    triangle_gain: f32,
    sine_gain: f32,
    sub_gain: f32,
    output_gain: f32,
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
    samples_until_control: u8,
    control: HeldControl,
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
            samples_until_control: 0,
            control: HeldControl::default(),
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
        self.samples_until_control = 0;
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

    #[inline]
    fn render_sample(
        &mut self,
        sample_rate_hz: f32,
        params: &EngineParams,
        lfo_levels: &[f32; 2],
    ) -> f32 {
        if !self.is_active() {
            return 0.0;
        }
        if self.samples_until_control == 0 {
            self.update_control(sample_rate_hz, params, lfo_levels);
            self.samples_until_control = CONTROL_BLOCK_SAMPLES as u8;
        }
        self.samples_until_control -= 1;

        let mut mix = 0.0;
        if self.control.saw_gain > 0.0 {
            mix += self.control.saw_gain * self.saw.next_saw();
        }
        if self.control.square_gain > 0.0 {
            mix += self.control.square_gain * self.square.next_square();
        }
        if self.control.triangle_gain > 0.0 {
            mix += self.control.triangle_gain * self.triangle.next_triangle();
        }
        if self.control.sine_gain > 0.0 {
            mix += self.control.sine_gain * self.sine.next_sine();
        }
        if self.control.sub_gain > 0.0 {
            mix += self.control.sub_gain * self.sub.next_sine();
        }
        let filtered = self.filter.tick(mix);
        let amp = self.amp.next_level();
        filtered * amp * self.control.output_gain
    }

    fn update_control(
        &mut self,
        sample_rate_hz: f32,
        params: &EngineParams,
        lfo_levels: &[f32; 2],
    ) {
        let (filter_level, assign_level) = self.advance_control_envelopes();
        let velocity = self.velocity_amp;
        let filter_octaves = filter_level
            * effective_envelope_amount(params.filter_env_amount, params.env_vel, velocity);
        let assign_amount =
            effective_envelope_amount(params.assignable_amount, params.env_vel, velocity);

        let mut offsets = ModOffsets::default();
        add_assignable(
            &mut offsets,
            params.assignable_dest,
            assign_level,
            assign_amount,
        );
        for (index, level) in lfo_levels.iter().copied().enumerate() {
            let lfo_params = &params.lfos[index];
            if lfo_params.dest == AssignableDest::Off || lfo_params.amount == 0.0 {
                continue;
            }
            add_assignable(&mut offsets, lfo_params.dest, level, lfo_params.amount);
        }
        offsets.cutoff_octaves += filter_octaves;

        let oscillator_hz = hz_times_octaves(self.base_hz, offsets.pitch_octaves)
            .clamp(20.0, sample_rate_hz * 0.25);
        let pulse_width =
            (params.pulse_width + offsets.pulse_width).clamp(PULSE_WIDTH_MIN, PULSE_WIDTH_MAX);
        self.saw.set_frequency(sample_rate_hz, oscillator_hz);
        self.square.set_frequency(sample_rate_hz, oscillator_hz);
        self.square.set_pulse_width(pulse_width);
        self.triangle.set_frequency(sample_rate_hz, oscillator_hz);
        self.sine.set_frequency(sample_rate_hz, oscillator_hz);
        let sub_hz = (oscillator_hz / params.sub_octaves.frequency_divisor())
            .clamp(20.0, sample_rate_hz * 0.25);
        self.sub.set_frequency(sample_rate_hz, sub_hz);
        let cutoff_hz = hz_times_octaves(params.cutoff_hz, offsets.cutoff_octaves);
        let resonance = params.resonance + offsets.resonance;
        self.filter
            .update_coefficients(sample_rate_hz, cutoff_hz, resonance);

        let sum = params.saw_vol + params.square_vol + params.triangle_vol + params.sine_vol;
        let scale = if sum == 0.0 { 0.0 } else { 1.0 / sum };
        self.control.saw_gain = params.saw_vol * scale;
        self.control.square_gain = params.square_vol * scale;
        self.control.triangle_gain = params.triangle_vol * scale;
        self.control.sine_gain = params.sine_vol * scale;
        self.control.sub_gain = params.sub_vol;
        self.control.output_gain =
            self.velocity_amp * VOICE_AMPLITUDE * (1.0 + offsets.amp).max(0.0);
    }

    /// Moves the slow envelopes one block ahead and returns the levels used for this block.
    fn advance_control_envelopes(&mut self) -> (f32, f32) {
        let filter_level = self.filter_env.next_level();
        let assign_level = self.assignable_env.next_level();
        for _ in 1..CONTROL_BLOCK_SAMPLES {
            let _ = self.filter_env.next_level();
            let _ = self.assignable_env.next_level();
        }
        (filter_level, assign_level)
    }
}

pub(crate) struct Voices {
    sample_rate_hz: f32,
    voices: [Voice; VOICE_COUNT],
    lfos: [Lfo; 2],
    /// Level held for the current 32-sample block. Every voice reads this.
    lfo_levels: [f32; 2],
    samples_until_lfo: u8,
    next_age: u32,
}

impl Voices {
    pub(crate) fn new(sample_rate_hz: f32, params: &EngineParams) -> Self {
        let mut voices = Self {
            sample_rate_hz,
            voices: core::array::from_fn(|_| Voice::new(sample_rate_hz)),
            lfos: core::array::from_fn(|index| Lfo::new(sample_rate_hz, index)),
            lfo_levels: [0.0; 2],
            samples_until_lfo: 0,
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
        for (lfo_index, lfo) in self.lfos.iter_mut().enumerate() {
            if params.lfos[lfo_index].retrigger {
                lfo.retrigger();
            }
        }
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

    #[inline]
    pub(crate) fn render_sample(&mut self, params: &EngineParams) -> f32 {
        self.tick_lfos(params);
        let levels = self.lfo_levels;
        let sample_rate_hz = self.sample_rate_hz;
        self.voices
            .iter_mut()
            .map(|voice| voice.render_sample(sample_rate_hz, params, &levels))
            .sum()
    }

    /// Both LFOs step once per 32-sample block, including while no note is held.
    fn tick_lfos(&mut self, params: &EngineParams) {
        if self.samples_until_lfo == 0 {
            for (index, lfo) in self.lfos.iter_mut().enumerate() {
                let lfo_params = &params.lfos[index];
                self.lfo_levels[index] = lfo.advance(
                    lfo_params.rate_hz,
                    lfo_params.wave,
                    CONTROL_BLOCK_SAMPLES as u32,
                );
            }
            self.samples_until_lfo = CONTROL_BLOCK_SAMPLES as u8;
        }
        self.samples_until_lfo -= 1;
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
