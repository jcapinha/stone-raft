#![cfg_attr(not(test), no_std)]

mod envelope;
mod filter;
mod mixer;
mod oscillator;

pub use envelope::{Adsr, AdsrTimes, EnvelopeStage, velocity_to_amp};
pub use filter::Svf;
pub use mixer::{ENGINE_COUNT, Mixer, MixerEvent, SlotEvent};
pub use oscillator::{Oscillator, Waveform};

use envelope::velocity_to_amp as vel_amp;
use filter::Svf as VoiceFilter;
use oscillator::{Oscillator as VoiceOsc, Waveform as VoiceWave};

/// Fixed number of simultaneous voices per engine instance.
pub const VOICE_COUNT: usize = 4;

/// Conservative per-voice gain so a few bright voices stay near full scale.
const VOICE_AMPLITUDE: f32 = 0.12;

const AMT_MIN: f32 = -8.0;
const AMT_MAX: f32 = 8.0;

/// Destination for the assignable envelope. More destinations can be added later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env3Dest {
    Off,
    Resonance,
    Pitch,
    Cutoff,
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
    SetEnvelope {
        which: EnvelopeId,
        times: AdsrTimes,
    },
    PatchEnvelope {
        which: EnvelopeId,
        field: EnvelopeField,
        value: f32,
    },
    SetFiltEnvAmt {
        amount: f32,
    },
    SetEnv3Dest {
        dest: Env3Dest,
    },
    SetEnv3Amt {
        amount: f32,
    },
    EnvCopy,
    SetEnvLink {
        on: bool,
    },
    SetEnvVel {
        amount: f32,
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

fn effective_envelope_amount(amount: f32, envvel: f32, vel: f32) -> f32 {
    amount * (1.0 - envvel + envvel * vel)
}

struct Voice {
    oscillator: VoiceOsc,
    filter: VoiceFilter,
    amp: Adsr,
    filter_env: Adsr,
    assign_env: Adsr,
    note: u8,
    velocity_amp: f32,
    base_hz: f32,
    /// Monotonic age stamp; higher means more recently started (used for steal).
    age: u32,
}

impl Voice {
    fn new(sample_rate_hz: f32, waveform: VoiceWave) -> Self {
        Self {
            oscillator: VoiceOsc::new(sample_rate_hz, 440.0, waveform),
            filter: VoiceFilter::new(),
            amp: Adsr::new(sample_rate_hz),
            filter_env: Adsr::new(sample_rate_hz),
            assign_env: Adsr::new(sample_rate_hz),
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
}

/// Shared subtractive params for one engine instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineParams {
    pub waveform: Waveform,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub amp: AdsrTimes,
    pub filter_env: AdsrTimes,
    pub assign_env: AdsrTimes,
    pub filtenv_amt: f32,
    pub env3_amt: f32,
    pub env3_dest: Env3Dest,
    pub env_link: bool,
    pub envvel: f32,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            waveform: Waveform::Saw,
            cutoff_hz: 2_000.0,
            resonance: 0.2,
            amp: AdsrTimes::default(),
            filter_env: AdsrTimes::default(),
            assign_env: AdsrTimes::default(),
            filtenv_amt: 0.0,
            env3_amt: 0.0,
            env3_dest: Env3Dest::Off,
            env_link: false,
            envvel: 0.0,
        }
    }
}

impl EngineParams {
    pub fn envelope(&self, which: EnvelopeId) -> AdsrTimes {
        match which {
            EnvelopeId::Amp => self.amp,
            EnvelopeId::Filter => self.filter_env,
            EnvelopeId::Assignable => self.assign_env,
        }
    }

    /// Updates params for param events. Returns false for NoteOn/NoteOff.
    pub fn apply(&mut self, event: ControlEvent) -> bool {
        match event {
            ControlEvent::NoteOn { .. } | ControlEvent::NoteOff { .. } => false,
            ControlEvent::SetCutoff { hz } => {
                self.cutoff_hz = hz.max(20.0);
                true
            }
            ControlEvent::SetResonance { amount } => {
                self.resonance = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetWave { waveform } => {
                self.waveform = waveform;
                true
            }
            ControlEvent::SetEnvelope { which, times } => {
                self.set_envelope(which, times);
                true
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
                true
            }
            ControlEvent::SetFiltEnvAmt { amount } => {
                self.filtenv_amt = amount.clamp(AMT_MIN, AMT_MAX);
                true
            }
            ControlEvent::SetEnv3Dest { dest } => {
                self.env3_dest = dest;
                true
            }
            ControlEvent::SetEnv3Amt { amount } => {
                self.env3_amt = amount.clamp(AMT_MIN, AMT_MAX);
                true
            }
            ControlEvent::EnvCopy => {
                self.copy_amp_times_to_extra_envelopes();
                true
            }
            ControlEvent::SetEnvLink { on } => {
                self.env_link = on;
                if on {
                    self.copy_amp_times_to_extra_envelopes();
                }
                true
            }
            ControlEvent::SetEnvVel { amount } => {
                self.envvel = amount.clamp(0.0, 1.0);
                true
            }
        }
    }

    fn set_envelope(&mut self, which: EnvelopeId, times: AdsrTimes) {
        let times = times.clamped();
        match which {
            EnvelopeId::Amp => {
                self.amp = times;
                if self.env_link {
                    self.filter_env = times;
                    self.assign_env = times;
                }
            }
            EnvelopeId::Filter => {
                self.unlink_if_needed();
                self.filter_env = times;
            }
            EnvelopeId::Assignable => {
                self.unlink_if_needed();
                self.assign_env = times;
            }
        }
    }

    fn unlink_if_needed(&mut self) {
        if self.env_link {
            self.env_link = false;
        }
    }

    fn copy_amp_times_to_extra_envelopes(&mut self) {
        self.filter_env = self.amp;
        self.assign_env = self.amp;
    }
}

/// One engine instance: fixed voices that turn MIDI notes into mono audio.
pub struct Engine {
    sample_rate_hz: f32,
    voices: [Voice; VOICE_COUNT],
    next_age: u32,
    params: EngineParams,
    #[cfg(test)]
    next_sample_calls: u32,
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
            #[cfg(test)]
            next_sample_calls: 0,
        };
        engine.apply_envelope_params_to_all();
        engine
    }

    pub fn params(&self) -> &EngineParams {
        &self.params
    }

    /// Applies one note or param change. Call only from the audio thread.
    pub fn apply(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::NoteOn { note, velocity } => self.note_on(note, velocity),
            ControlEvent::NoteOff { note } => self.note_off(note),
            ControlEvent::SetWave { waveform } => {
                let _ = self.params.apply(event);
                for voice in self.voices.iter_mut() {
                    voice.oscillator.set_waveform(waveform);
                }
            }
            ControlEvent::SetEnvelope { .. }
            | ControlEvent::PatchEnvelope { .. }
            | ControlEvent::EnvCopy => {
                let _ = self.params.apply(event);
                self.apply_envelope_params_to_all();
            }
            ControlEvent::SetEnvLink { on } => {
                let _ = self.params.apply(event);
                if on {
                    self.apply_envelope_params_to_all();
                }
            }
            ControlEvent::SetCutoff { .. }
            | ControlEvent::SetResonance { .. }
            | ControlEvent::SetFiltEnvAmt { .. }
            | ControlEvent::SetEnv3Dest { .. }
            | ControlEvent::SetEnv3Amt { .. }
            | ControlEvent::SetEnvVel { .. } => {
                let _ = self.params.apply(event);
            }
        }
    }

    /// Force every voice envelope to Idle at level 0 so sound does not resume later.
    pub fn silence(&mut self) {
        for voice in self.voices.iter_mut() {
            voice.amp.force_idle();
            voice.filter_env.force_idle();
            voice.assign_env.force_idle();
        }
    }

    #[cfg(test)]
    pub(crate) fn next_sample_call_count(&self) -> u32 {
        self.next_sample_calls
    }

    fn apply_envelope_params_to_all(&mut self) {
        let params = &self.params;
        for voice in self.voices.iter_mut() {
            apply_envelope_params_to_voice(voice, params);
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
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

    fn note_off(&mut self, note: u8) {
        for voice in self.voices.iter_mut() {
            if voice.is_active() && voice.note == note {
                voice.amp.note_off();
                voice.filter_env.note_off();
                voice.assign_env.note_off();
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
        let base_hz = midi_note_to_hz(note);

        let voice = &mut self.voices[index];
        voice.oscillator = VoiceOsc::new(sample_rate, base_hz, waveform);
        voice.filter.reset();
        apply_envelope_params_to_voice(voice, &self.params);
        voice.amp.note_on();
        voice.filter_env.note_on();
        voice.assign_env.note_on();
        voice.note = note;
        voice.velocity_amp = vel_amp(velocity);
        voice.base_hz = base_hz;
        voice.age = age;
    }

    /// Sums active voices into one mono sample in roughly [-1.0, 1.0].
    pub fn next_sample(&mut self) -> f32 {
        #[cfg(test)]
        {
            self.next_sample_calls = self.next_sample_calls.wrapping_add(1);
        }
        let sample_rate = self.sample_rate_hz;
        let cutoff_base = self.params.cutoff_hz;
        let resonance_base = self.params.resonance;
        let filtenv_amt = self.params.filtenv_amt;
        let env3_amt = self.params.env3_amt;
        let env3_dest = self.params.env3_dest;
        let envvel = self.params.envvel;

        let mut mix = 0.0;
        for voice in self.voices.iter_mut() {
            if !voice.is_active() {
                continue;
            }

            let filt_level = voice.filter_env.next_level();
            let assign_level = voice.assign_env.next_level();
            let vel = voice.velocity_amp;
            let filt_oct = filt_level * effective_envelope_amount(filtenv_amt, envvel, vel);
            let env3_effective = effective_envelope_amount(env3_amt, envvel, vel);

            let (env3_cutoff_oct, resonance, osc_hz) = match env3_dest {
                Env3Dest::Off => (0.0, resonance_base, voice.base_hz),
                Env3Dest::Resonance => (
                    0.0,
                    resonance_base + assign_level * env3_effective,
                    voice.base_hz,
                ),
                Env3Dest::Pitch => {
                    let hz = hz_times_octaves(voice.base_hz, assign_level * env3_effective);
                    let hz = hz.clamp(20.0, sample_rate * 0.25);
                    (0.0, resonance_base, hz)
                }
                Env3Dest::Cutoff => (assign_level * env3_effective, resonance_base, voice.base_hz),
            };

            let cutoff_hz = hz_times_octaves(cutoff_base, filt_oct + env3_cutoff_oct);
            voice.oscillator.set_frequency(sample_rate, osc_hz);
            let osc = voice.oscillator.next_sample();
            let filtered = voice.filter.process(osc, sample_rate, cutoff_hz, resonance);
            let amp = voice.amp.next_level();
            mix += filtered * amp * voice.velocity_amp * VOICE_AMPLITUDE;
        }
        mix
    }
}

fn apply_envelope_params_to_voice(voice: &mut Voice, params: &EngineParams) {
    params.amp.apply_to(&mut voice.amp);
    params.filter_env.apply_to(&mut voice.filter_env);
    params.assign_env.apply_to(&mut voice.assign_env);
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
        let mut times = engine.params().amp;
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

    fn set_fast_assign_env_sustain(engine: &mut Engine) {
        let mut times = engine.params().assign_env;
        times.attack_ms = 1.0;
        times.decay_ms = 1.0;
        times.sustain = 1.0;
        set_envelope(engine, EnvelopeId::Assignable, times);
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
        closed.apply(ControlEvent::SetFiltEnvAmt { amount: 0.0 });
        opened.apply(ControlEvent::SetFiltEnvAmt { amount: 4.0 });

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
            set_fast_assign_env_sustain(engine);
        }
        stacked.apply(ControlEvent::SetEnv3Dest {
            dest: Env3Dest::Cutoff,
        });
        stacked.apply(ControlEvent::SetFiltEnvAmt { amount: 2.0 });
        stacked.apply(ControlEvent::SetEnv3Amt { amount: 2.0 });
        combined.apply(ControlEvent::SetFiltEnvAmt { amount: 4.0 });
        combined.apply(ControlEvent::SetEnv3Amt { amount: 0.0 });

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
        set_fast_assign_env_sustain(&mut engine);
        engine.apply(ControlEvent::SetEnv3Dest {
            dest: Env3Dest::Pitch,
        });
        engine.apply(ControlEvent::SetEnv3Amt { amount: 1.0 });

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
        assert!((engine.params().assign_env.attack_ms - 50.0).abs() < f32::EPSILON);
        assert!(!engine.params().env_link);

        engine.apply(ControlEvent::SetEnvLink { on: true });
        assert!(engine.params().env_link);
        patch_field(&mut engine, EnvelopeId::Amp, EnvelopeField::Attack, 90.0);
        assert!((engine.params().filter_env.attack_ms - 90.0).abs() < f32::EPSILON);
        assert!((engine.params().assign_env.attack_ms - 90.0).abs() < f32::EPSILON);

        patch_field(&mut engine, EnvelopeId::Filter, EnvelopeField::Attack, 12.0);
        assert!(!engine.params().env_link);
        assert!((engine.params().filter_env.attack_ms - 12.0).abs() < f32::EPSILON);
        patch_field(&mut engine, EnvelopeId::Amp, EnvelopeField::Attack, 200.0);
        assert!((engine.params().amp.attack_ms - 200.0).abs() < f32::EPSILON);
        assert!((engine.params().filter_env.attack_ms - 12.0).abs() < f32::EPSILON);

        engine.apply(ControlEvent::SetEnvLink { on: true });
        patch_field(
            &mut engine,
            EnvelopeId::Assignable,
            EnvelopeField::Decay,
            33.0,
        );
        assert!(!engine.params().env_link);
        assert!((engine.params().assign_env.decay_ms - 33.0).abs() < f32::EPSILON);
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
        let amp = engine.params().amp;
        assert_eq!(amp.attack_ms, 0.0);
        assert_eq!(amp.decay_ms, 0.0);
        assert_eq!(amp.sustain, 1.0);
        assert_eq!(amp.release_ms, 0.0);
    }

    #[test]
    fn envvel_scales_pitch_dest_by_velocity() {
        let mut quiet = Engine::new(SAMPLE_RATE_HZ);
        let mut loud = Engine::new(SAMPLE_RATE_HZ);
        for engine in [&mut quiet, &mut loud] {
            set_wave(engine, Waveform::Saw);
            set_cutoff(engine, 10_000.0);
            set_res(engine, 0.0);
            set_fast_amp_sustain(engine);
            set_fast_assign_env_sustain(engine);
            engine.apply(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Pitch,
            });
            engine.apply(ControlEvent::SetEnv3Amt { amount: 1.0 });
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
            "low velocity with envvel 1 should stay nearer the original pitch; base={quiet_at_base} octave={quiet_at_octave}"
        );
    }
}
