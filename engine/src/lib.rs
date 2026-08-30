#![cfg_attr(not(test), no_std)]

mod envelope;
mod filter;
mod mixer;
mod oscillator;

pub use envelope::{Adsr, AdsrTimes, EnvelopeStage, velocity_to_amp};
pub use filter::Svf;
pub use mixer::{ENGINE_COUNT, Mixer, MixerEvent, SlotEvent};
pub use oscillator::{
    Oscillator, PULSE_WIDTH_DEFAULT, PULSE_WIDTH_MAX, PULSE_WIDTH_MIN, Waveform,
};

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

fn effective_envelope_amount(amount: f32, envvel: f32, vel: f32) -> f32 {
    amount * (1.0 - envvel + envvel * vel)
}

#[derive(Clone, Copy)]
struct AtPitchLevels {
    saw: f32,
    square: f32,
    triangle: f32,
    sine: f32,
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

struct Voice {
    saw: VoiceOsc,
    square: VoiceOsc,
    triangle: VoiceOsc,
    sine: VoiceOsc,
    sub: VoiceOsc,
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
    fn new(sample_rate_hz: f32) -> Self {
        Self {
            saw: VoiceOsc::new(sample_rate_hz, 440.0, VoiceWave::Saw),
            square: VoiceOsc::new(sample_rate_hz, 440.0, VoiceWave::Square),
            triangle: VoiceOsc::new(sample_rate_hz, 440.0, VoiceWave::Triangle),
            sine: VoiceOsc::new(sample_rate_hz, 440.0, VoiceWave::Sine),
            sub: VoiceOsc::new(sample_rate_hz, 220.0, VoiceWave::Sine),
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
    pub saw_vol: f32,
    pub square_vol: f32,
    pub triangle_vol: f32,
    pub sine_vol: f32,
    pub pulse_width: f32,
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
    pub sub_vol: f32,
    pub sub_octaves: SubOctaves,
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
            amp: AdsrTimes::default(),
            filter_env: AdsrTimes::default(),
            assign_env: AdsrTimes::default(),
            filtenv_amt: 0.0,
            env3_amt: 0.0,
            env3_dest: Env3Dest::Off,
            env_link: false,
            envvel: 0.0,
            sub_vol: 0.0,
            sub_octaves: SubOctaves::One,
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
                true
            }
            ControlEvent::SetSawVol { amount } => {
                self.saw_vol = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetSquareVol { amount } => {
                self.square_vol = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetTriangleVol { amount } => {
                self.triangle_vol = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetSineVol { amount } => {
                self.sine_vol = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetPulse { width } => {
                self.pulse_width = width.clamp(PULSE_WIDTH_MIN, PULSE_WIDTH_MAX);
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
            ControlEvent::SetSubVol { amount } => {
                self.sub_vol = amount.clamp(0.0, 1.0);
                true
            }
            ControlEvent::SetSubOct { octaves } => {
                self.sub_octaves = octaves;
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
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
                Voice::new(sample_rate_hz),
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
            ControlEvent::SetPulse { width } => {
                let _ = self.params.apply(event);
                for voice in self.voices.iter_mut() {
                    voice.square.set_pulse_width(width);
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
            | ControlEvent::SetWave { .. }
            | ControlEvent::SetSawVol { .. }
            | ControlEvent::SetSquareVol { .. }
            | ControlEvent::SetTriangleVol { .. }
            | ControlEvent::SetSineVol { .. }
            | ControlEvent::SetFiltEnvAmt { .. }
            | ControlEvent::SetEnv3Dest { .. }
            | ControlEvent::SetEnv3Amt { .. }
            | ControlEvent::SetEnvVel { .. }
            | ControlEvent::SetSubVol { .. }
            | ControlEvent::SetSubOct { .. } => {
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
        let pulse_width = self.params.pulse_width;
        let base_hz = midi_note_to_hz(note);

        let voice = &mut self.voices[index];
        voice.saw = VoiceOsc::new(sample_rate, base_hz, VoiceWave::Saw);
        voice.square = VoiceOsc::new(sample_rate, base_hz, VoiceWave::Square);
        voice.square.set_pulse_width(pulse_width);
        voice.triangle = VoiceOsc::new(sample_rate, base_hz, VoiceWave::Triangle);
        voice.sine = VoiceOsc::new(sample_rate, base_hz, VoiceWave::Sine);
        let sub_hz = (base_hz / self.params.sub_octaves.frequency_divisor())
            .clamp(20.0, sample_rate * 0.25);
        voice.sub = VoiceOsc::new(sample_rate, sub_hz, VoiceWave::Sine);
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
        let sub_vol = self.params.sub_vol;
        let sub_octaves = self.params.sub_octaves;
        let at_pitch = AtPitchLevels {
            saw: self.params.saw_vol,
            square: self.params.square_vol,
            triangle: self.params.triangle_vol,
            sine: self.params.sine_vol,
        };

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

            if at_pitch.saw > 0.0 {
                voice.saw.set_frequency(sample_rate, osc_hz);
            }
            if at_pitch.square > 0.0 {
                voice.square.set_frequency(sample_rate, osc_hz);
            }
            if at_pitch.triangle > 0.0 {
                voice.triangle.set_frequency(sample_rate, osc_hz);
            }
            if at_pitch.sine > 0.0 {
                voice.sine.set_frequency(sample_rate, osc_hz);
            }

            let saw_sample = if at_pitch.saw > 0.0 {
                voice.saw.next_sample()
            } else {
                0.0
            };
            let square_sample = if at_pitch.square > 0.0 {
                voice.square.next_sample()
            } else {
                0.0
            };
            let triangle_sample = if at_pitch.triangle > 0.0 {
                voice.triangle.next_sample()
            } else {
                0.0
            };
            let sine_sample = if at_pitch.sine > 0.0 {
                voice.sine.next_sample()
            } else {
                0.0
            };
            let main = normalize_blend(
                at_pitch,
                [saw_sample, square_sample, triangle_sample, sine_sample],
            );
            let osc = if sub_vol > 0.0 {
                let sub_hz = (osc_hz / sub_octaves.frequency_divisor())
                    .clamp(20.0, sample_rate * 0.25);
                voice.sub.set_frequency(sample_rate, sub_hz);
                main + sub_vol * voice.sub.next_sample()
            } else {
                main
            };
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
        for waveform in [
            Waveform::Square,
            Waveform::Triangle,
            Waveform::Sine,
        ] {
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
    fn sub_tracks_env3_pitch_destination() {
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
}
