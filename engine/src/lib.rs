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

    #[test]
    fn fifth_note_steals_oldest() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        engine.note_on(60, 100);
        engine.note_on(62, 100);
        engine.note_on(64, 100);
        engine.note_on(65, 100);
        engine.note_on(67, 100);

        let active_notes: Vec<u8> = engine
            .voices
            .iter()
            .filter(|v| v.active)
            .map(|v| v.note)
            .collect();

        assert_eq!(active_notes.len(), VOICE_COUNT);
        assert!(!active_notes.contains(&60), "oldest note 60 should be stolen");
        assert!(active_notes.contains(&67));
        assert!(active_notes.contains(&62));
        assert!(active_notes.contains(&64));
        assert!(active_notes.contains(&65));
    }

    #[test]
    fn note_off_silences_matching_voice() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        engine.note_on(60, 100);
        engine.note_on(64, 100);
        engine.note_off(60);

        let active: Vec<u8> = engine
            .voices
            .iter()
            .filter(|v| v.active)
            .map(|v| v.note)
            .collect();
        assert_eq!(active, vec![64]);
    }

    #[test]
    fn velocity_is_stored_unused_for_gain() {
        let mut engine = Engine::new(SAMPLE_RATE_HZ);
        engine.note_on(60, 77);
        let voice = engine.voices.iter().find(|v| v.active).expect("voice on");
        assert_eq!(voice.velocity, 77);
        assert_eq!(voice.note, 60);
    }
}
