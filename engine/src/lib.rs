#![cfg_attr(not(test), no_std)]

use core::f32::consts::TAU;

/// Fixed number of simultaneous voices per engine instance.
pub const VOICE_COUNT: usize = 4;

/// Conservative per-voice gain so a few summed sines stay near full scale.
const VOICE_AMPLITUDE: f32 = 0.15;

/// Converts a MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
pub fn midi_note_to_hz(note: u8) -> f32 {
    let semitones_from_a4 = f32::from(note) - 69.0;
    440.0 * libm::powf(2.0, semitones_from_a4 / 12.0)
}

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

struct Voice {
    oscillator: Oscillator,
    active: bool,
    note: u8,
    /// Stored for later dynamics; not applied to gain yet.
    velocity: u8,
    /// Monotonic age stamp; higher means more recently started (used for steal-oldest).
    age: u32,
}

impl Voice {
    fn new(sample_rate_hz: f32) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate_hz, 440.0),
            active: false,
            note: 0,
            velocity: 0,
            age: 0,
        }
    }
}

/// One engine instance: a fixed pool of voices that turn MIDI notes into mono audio.
pub struct Engine {
    sample_rate_hz: f32,
    voices: [Voice; VOICE_COUNT],
    next_age: u32,
}

impl Engine {
    pub fn new(sample_rate_hz: f32) -> Self {
        Self {
            sample_rate_hz,
            voices: [
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
            ],
            next_age: 1,
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }

        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);

        if let Some(index) = self
            .voices
            .iter()
            .position(|v| v.active && v.note == note)
        {
            start_voice(
                &mut self.voices[index],
                self.sample_rate_hz,
                note,
                velocity,
                age,
            );
            return;
        }

        if let Some(index) = self.voices.iter().position(|v| !v.active) {
            start_voice(
                &mut self.voices[index],
                self.sample_rate_hz,
                note,
                velocity,
                age,
            );
            return;
        }

        let oldest_index = self
            .voices
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.age)
            .map(|(i, _)| i)
            .expect("VOICE_COUNT is non-zero");
        start_voice(
            &mut self.voices[oldest_index],
            self.sample_rate_hz,
            note,
            velocity,
            age,
        );
    }

    pub fn note_off(&mut self, note: u8) {
        for voice in self.voices.iter_mut() {
            if voice.active && voice.note == note {
                voice.active = false;
            }
        }
    }

    /// Sums active voices into one mono sample in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        let mut mix = 0.0;
        for voice in self.voices.iter_mut() {
            if voice.active {
                mix += voice.oscillator.next_sample() * VOICE_AMPLITUDE;
            }
        }
        mix
    }
}

fn start_voice(voice: &mut Voice, sample_rate_hz: f32, note: u8, velocity: u8, age: u32) {
    voice.oscillator = Oscillator::new(sample_rate_hz, midi_note_to_hz(note));
    voice.active = true;
    voice.note = note;
    voice.velocity = velocity;
    voice.age = age;
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

    /// How strongly `frequency_hz` appears in `samples` (normalized DFT bin magnitude).
    /// Present sines at `VOICE_AMPLITUDE` score near half that amplitude; absent pitches score near 0.
    fn tone_strength(samples: &[f32], frequency_hz: f32) -> f32 {
        let mut re = 0.0f32;
        let mut im = 0.0f32;
        for (n, &sample) in samples.iter().enumerate() {
            let phase = TAU * frequency_hz * (n as f32) / SAMPLE_RATE_HZ;
            re += sample * phase.cos();
            im += sample * phase.sin();
        }
        (re * re + im * im).sqrt() / samples.len() as f32
    }

    fn take_samples(engine: &mut Engine, count: usize) -> Vec<f32> {
        (0..count).map(|_| engine.next_sample()).collect()
    }

    /// Below this, a pitch is treated as absent from the mix; above, as present.
    /// Tuned for VOICE_AMPLITUDE and a few-thousand-sample window at 48 kHz.
    const TONE_PRESENT: f32 = 0.04;
    const TONE_ABSENT: f32 = 0.015;
    const ANALYSIS_SAMPLES: usize = 4096;

    #[test]
    fn fifth_note_steals_oldest() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        engine.note_on(60, 100);
        engine.note_on(62, 100);
        engine.note_on(64, 100);
        engine.note_on(65, 100);
        engine.note_on(67, 100);

        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);

        assert!(
            tone_strength(&samples, midi_note_to_hz(60)) < TONE_ABSENT,
            "oldest note 60 should be stolen"
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
    fn note_off_silences_matching_voice() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        engine.note_on(60, 100);
        engine.note_on(64, 100);
        engine.note_off(60);

        let samples = take_samples(&mut engine, ANALYSIS_SAMPLES);
        let strength_60 = tone_strength(&samples, midi_note_to_hz(60));
        let strength_64 = tone_strength(&samples, midi_note_to_hz(64));

        assert!(
            strength_60 < TONE_ABSENT,
            "note 60 should be silent after note_off, strength was {strength_60}"
        );
        assert!(
            strength_64 > TONE_PRESENT,
            "note 64 should still sound, strength was {strength_64}"
        );
    }

    #[test]
    fn velocity_does_not_affect_gain() {
        let mut quiet = Engine::new(SAMPLE_RATE_HZ);
        let mut loud = Engine::new(SAMPLE_RATE_HZ);
        quiet.note_on(60, 1);
        loud.note_on(60, 127);

        let mut peak_quiet = 0.0f32;
        let mut peak_loud = 0.0f32;
        for _ in 0..ANALYSIS_SAMPLES {
            peak_quiet = peak_quiet.max(quiet.next_sample().abs());
            peak_loud = peak_loud.max(loud.next_sample().abs());
        }

        let diff = (peak_loud - peak_quiet).abs();
        assert!(
            diff < 1e-5,
            "velocity should not change loudness yet; peak quiet={peak_quiet}, loud={peak_loud}"
        );
    }
}
