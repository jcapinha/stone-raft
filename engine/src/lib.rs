#![cfg_attr(not(test), no_std)]

mod envelope;
mod filter;
mod oscillator;

pub use envelope::{velocity_to_amp, Adsr, EnvelopeStage};
pub use filter::Svf;
pub use oscillator::{Oscillator, Waveform};

use envelope::velocity_to_amp as vel_amp;
use filter::Svf as VoiceFilter;
use oscillator::{Oscillator as VoiceOsc, Waveform as VoiceWave};

/// Fixed number of simultaneous voices per engine instance.
pub const VOICE_COUNT: usize = 4;

/// Conservative per-voice gain so a few bright voices stay near full scale.
const VOICE_AMPLITUDE: f32 = 0.12;

/// Converts a MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
pub fn midi_note_to_hz(note: u8) -> f32 {
    let semitones_from_a4 = f32::from(note) - 69.0;
    440.0 * libm::powf(2.0, semitones_from_a4 / 12.0)
}

struct Voice {
    oscillator: VoiceOsc,
    filter: VoiceFilter,
    amp: Adsr,
    note: u8,
    velocity_amp: f32,
    /// Monotonic age stamp; higher means more recently started (used for steal).
    age: u32,
}

impl Voice {
    fn new(sample_rate_hz: f32, waveform: VoiceWave) -> Self {
        Self {
            oscillator: VoiceOsc::new(sample_rate_hz, 440.0, waveform),
            filter: VoiceFilter::new(),
            amp: Adsr::new(sample_rate_hz),
            note: 0,
            velocity_amp: 1.0,
            age: 0,
        }
    }

    fn is_active(&self) -> bool {
        self.amp.is_active()
    }

    fn is_releasing(&self) -> bool {
        self.amp.is_releasing()
    }
}

/// Shared subtractive params for one engine instance.
pub struct EngineParams {
    pub waveform: Waveform,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            waveform: Waveform::Saw,
            cutoff_hz: 2_000.0,
            resonance: 0.2,
            attack_ms: 10.0,
            decay_ms: 100.0,
            sustain: 0.7,
            release_ms: 200.0,
        }
    }
}

/// One engine instance: fixed voices that turn MIDI notes into mono audio.
pub struct Engine {
    sample_rate_hz: f32,
    voices: [Voice; VOICE_COUNT],
    next_age: u32,
    params: EngineParams,
}

impl Engine {
    pub fn new(sample_rate_hz: f32) -> Self {
        let params = EngineParams::default();
        let mut engine = Self {
            sample_rate_hz,
            voices: [
                Voice::new(sample_rate_hz, params.waveform),
                Voice::new(sample_rate_hz, params.waveform),
                Voice::new(sample_rate_hz, params.waveform),
                Voice::new(sample_rate_hz, params.waveform),
            ],
            next_age: 1,
            params,
        };
        engine.apply_envelope_params_to_all();
        engine
    }

    pub fn params(&self) -> &EngineParams {
        &self.params
    }

    pub fn set_waveform(&mut self, waveform: Waveform) {
        self.params.waveform = waveform;
        for voice in self.voices.iter_mut() {
            voice.oscillator.set_waveform(waveform);
        }
    }

    pub fn set_cutoff_hz(&mut self, cutoff_hz: f32) {
        self.params.cutoff_hz = cutoff_hz.max(20.0);
    }

    pub fn set_resonance(&mut self, resonance: f32) {
        self.params.resonance = resonance.clamp(0.0, 1.0);
    }

    pub fn set_attack_ms(&mut self, attack_ms: f32) {
        self.params.attack_ms = attack_ms.max(0.0);
        self.apply_envelope_params_to_all();
    }

    pub fn set_decay_ms(&mut self, decay_ms: f32) {
        self.params.decay_ms = decay_ms.max(0.0);
        self.apply_envelope_params_to_all();
    }

    pub fn set_sustain(&mut self, sustain: f32) {
        self.params.sustain = sustain.clamp(0.0, 1.0);
        self.apply_envelope_params_to_all();
    }

    pub fn set_release_ms(&mut self, release_ms: f32) {
        self.params.release_ms = release_ms.max(0.0);
        self.apply_envelope_params_to_all();
    }

    fn apply_envelope_params_to_all(&mut self) {
        let attack = self.params.attack_ms;
        let decay = self.params.decay_ms;
        let release = self.params.release_ms;
        let sustain = self.params.sustain;
        for voice in self.voices.iter_mut() {
            voice.amp.set_times_ms(attack, decay, release);
            voice.amp.set_sustain(sustain);
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
            .position(|v| v.is_active() && v.note == note)
        {
            self.start_voice(index, note, velocity, age);
            return;
        }

        if let Some(index) = self.voices.iter().position(|v| !v.is_active()) {
            self.start_voice(index, note, velocity, age);
            return;
        }

        let index = self.steal_index();
        self.start_voice(index, note, velocity, age);
    }

    pub fn note_off(&mut self, note: u8) {
        for voice in self.voices.iter_mut() {
            if voice.is_active() && voice.note == note {
                voice.amp.note_off();
            }
        }
    }

    fn steal_index(&self) -> usize {
        // Prefer oldest voice already in release; otherwise oldest overall.
        let releasing = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_releasing())
            .min_by_key(|(_, v)| v.age)
            .map(|(i, _)| i);

        if let Some(index) = releasing {
            return index;
        }

        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.age)
            .map(|(i, _)| i)
            .expect("VOICE_COUNT is non-zero")
    }

    fn start_voice(&mut self, index: usize, note: u8, velocity: u8, age: u32) {
        let sample_rate = self.sample_rate_hz;
        let waveform = self.params.waveform;
        let attack = self.params.attack_ms;
        let decay = self.params.decay_ms;
        let release = self.params.release_ms;
        let sustain = self.params.sustain;

        let voice = &mut self.voices[index];
        voice.oscillator = VoiceOsc::new(sample_rate, midi_note_to_hz(note), waveform);
        voice.filter.reset();
        voice.amp.set_times_ms(attack, decay, release);
        voice.amp.set_sustain(sustain);
        voice.amp.note_on();
        voice.note = note;
        voice.velocity_amp = vel_amp(velocity);
        voice.age = age;
    }

    /// Sums active voices into one mono sample in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        let sample_rate = self.sample_rate_hz;
        let cutoff = self.params.cutoff_hz;
        let resonance = self.params.resonance;

        let mut mix = 0.0;
        for voice in self.voices.iter_mut() {
            if !voice.is_active() {
                continue;
            }
            let osc = voice.oscillator.next_sample();
            let filtered = voice.filter.process(osc, sample_rate, cutoff, resonance);
            let amp = voice.amp.next_level();
            mix += filtered * amp * voice.velocity_amp * VOICE_AMPLITUDE;
        }
        mix
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
    fn velocity_curve_is_square_of_linear() {
        let linear_64 = 64.0 / 127.0;
        let curved = velocity_to_amp(64);
        assert!(
            (curved - linear_64 * linear_64).abs() < 1e-5,
            "expected square curve, got {curved}"
        );
        assert!(velocity_to_amp(127) > velocity_to_amp(64));
        assert!(velocity_to_amp(64) > velocity_to_amp(32));
        // Soft velocities drop more than linear: at 64, curve < linear.
        assert!(curved < linear_64);
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
        osc.set_waveform(Waveform::Square);
        for _ in 0..10_000 {
            let sample = osc.next_sample();
            assert!(
                (-1.5..=1.5).contains(&sample),
                "sample {sample} escaped a safe range"
            );
        }
    }

    #[test]
    fn higher_cutoff_passes_more_high_harmonic_energy() {
        // Square wave at 220 Hz; measure energy near the 5th harmonic (~1100 Hz).
        let mut dark = Engine::new(SAMPLE_RATE_HZ);
        let mut bright = Engine::new(SAMPLE_RATE_HZ);
        dark.set_waveform(Waveform::Square);
        bright.set_waveform(Waveform::Square);
        dark.set_cutoff_hz(400.0);
        bright.set_cutoff_hz(8_000.0);
        dark.set_resonance(0.0);
        bright.set_resonance(0.0);
        // Fast amp so analysis is mostly sustain.
        dark.set_attack_ms(1.0);
        bright.set_attack_ms(1.0);
        dark.set_decay_ms(1.0);
        bright.set_decay_ms(1.0);
        dark.set_sustain(1.0);
        bright.set_sustain(1.0);

        dark.note_on(57, 127); // A3 ≈ 220 Hz
        bright.note_on(57, 127);
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
        engine.set_attack_ms(1.0);
        engine.set_decay_ms(1.0);
        engine.set_sustain(1.0);
        engine.set_release_ms(50.0);
        engine.note_on(60, 127);
        for _ in 0..2_000 {
            engine.next_sample();
        }
        engine.note_off(60);

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
        engine.set_attack_ms(1.0);
        engine.set_decay_ms(1.0);
        engine.set_sustain(1.0);
        engine.set_release_ms(500.0);

        engine.note_on(60, 127);
        engine.note_on(62, 127);
        engine.note_on(64, 127);
        engine.note_on(65, 127);
        // Put the oldest note into release so it should be stolen first.
        engine.note_off(60);
        for _ in 0..100 {
            engine.next_sample();
        }
        engine.note_on(67, 127);

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
            engine.set_attack_ms(1.0);
            engine.set_decay_ms(1.0);
            engine.set_sustain(1.0);
            engine.set_cutoff_hz(8_000.0);
        }
        quiet.note_on(60, 40);
        loud.note_on(60, 127);

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
            engine.set_attack_ms(1.0);
            engine.set_decay_ms(1.0);
            engine.set_sustain(1.0);
            engine.set_cutoff_hz(10_000.0);
            engine.set_resonance(0.0);
        }
        saw.set_waveform(Waveform::Saw);
        square.set_waveform(Waveform::Square);
        saw.note_on(48, 127);
        square.note_on(48, 127);
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
}
