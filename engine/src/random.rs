//! Shared `random` patch recipe for laptop commands and Daisy firmware.

use rand::Rng;

use crate::{
    AdsrTimes, AssignableDest, ControlEvent, EngineParams, EnvelopeId, InstanceEvent,
    LFO_RATE_MAX_HZ, LFO_RATE_MIN_HZ, LfoId, LfoParams, LfoWave, MixerEvent, SubOctaves,
};

/// Enough slots for volume, osc mix, envelopes, both LFOs, and env link.
pub const PATCH_EVENT_MAX: usize = 32;

pub const RANDOM_CUTOFF_MIN_HZ: f32 = 80.0;
pub const RANDOM_CUTOFF_MAX_HZ: f32 = 12_000.0;
pub const RANDOM_RES_MAX: f32 = 0.9;
pub const RANDOM_TIME_MIN_MS: f32 = 1.0;
pub const RANDOM_TIME_MAX_MS: f32 = 2_000.0;
pub const RANDOM_AMT_MIN: f32 = -4.0;
pub const RANDOM_AMT_MAX: f32 = 4.0;
pub const RANDOM_RES_AMT_MIN: f32 = -1.0;
pub const RANDOM_RES_AMT_MAX: f32 = 1.0;
pub const RANDOM_VOL_MIN: f32 = 0.2;
pub const RANDOM_VOL_MAX: f32 = 1.0;
pub const RANDOM_PULSE_MIN: f32 = 0.05;
pub const RANDOM_PULSE_MAX: f32 = 0.95;
pub const RANDOM_PW_AMT_MIN: f32 = -0.4;
pub const RANDOM_PW_AMT_MAX: f32 = 0.4;
pub const RANDOM_AMP_AMT_MIN: f32 = -0.8;
pub const RANDOM_AMP_AMT_MAX: f32 = 0.8;

const RANDOM_ASSIGNABLE_DESTS: [AssignableDest; 6] = [
    AssignableDest::Off,
    AssignableDest::Resonance,
    AssignableDest::Pitch,
    AssignableDest::Cutoff,
    AssignableDest::PulseWidth,
    AssignableDest::Amp,
];
const RANDOM_LFO_WAVES: [LfoWave; 5] = [
    LfoWave::Sine,
    LfoWave::Triangle,
    LfoWave::Square,
    LfoWave::Saw,
    LfoWave::SampleHold,
];

/// Fixed buffer of mixer events for one `random` patch (volume plus params).
#[derive(Clone, Copy)]
pub struct PatchEvents {
    events: [MixerEvent; PATCH_EVENT_MAX],
    len: usize,
}

impl PatchEvents {
    pub fn as_slice(&self) -> &[MixerEvent] {
        &self.events[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

/// Random subtractive params and instance volume. Does not change on/off or listen channel.
pub fn random_patch<R: Rng>(rng: &mut R) -> (EngineParams, f32) {
    let amp = random_adsr(rng);
    let env_link = rng.gen_bool(0.5);
    let filter_env = if env_link { amp } else { random_adsr(rng) };
    let assign_env = if env_link { amp } else { random_adsr(rng) };
    let assignable_dest = RANDOM_ASSIGNABLE_DESTS[rng.gen_range(0..RANDOM_ASSIGNABLE_DESTS.len())];
    let params = EngineParams {
        saw_vol: rng.gen_range(0.0..=1.0),
        square_vol: rng.gen_range(0.0..=1.0),
        triangle_vol: rng.gen_range(0.0..=1.0),
        sine_vol: rng.gen_range(0.0..=1.0),
        pulse_width: rng.gen_range(RANDOM_PULSE_MIN..=RANDOM_PULSE_MAX),
        cutoff_hz: log_uniform(rng, RANDOM_CUTOFF_MIN_HZ, RANDOM_CUTOFF_MAX_HZ),
        resonance: rng.gen_range(0.0..=RANDOM_RES_MAX),
        amp_env: amp,
        filter_env,
        assignable_env: assign_env,
        filter_env_amount: rng.gen_range(RANDOM_AMT_MIN..=RANDOM_AMT_MAX),
        assignable_amount: random_amount_for_dest(rng, assignable_dest),
        assignable_dest,
        env_link,
        env_vel: rng.gen_range(0.0..=1.0),
        sub_vol: rng.gen_range(0.0..=1.0),
        sub_octaves: if rng.gen_bool(0.5) {
            SubOctaves::One
        } else {
            SubOctaves::Two
        },
        lfos: [random_lfo(rng), random_lfo(rng)],
    };
    let volume = rng.gen_range(RANDOM_VOL_MIN..=RANDOM_VOL_MAX);
    (params, volume)
}

/// Mixer events that load `params` and `volume` onto a 1-based instance.
pub fn patch_events(instance: u8, params: &EngineParams, volume: f32) -> PatchEvents {
    let mut events = [MixerEvent::MidiNoteOff {
        channel: 1,
        note: 0,
    }; PATCH_EVENT_MAX];
    let mut len = 0usize;
    let mut push = |event: MixerEvent| {
        events[len] = event;
        len += 1;
    };

    push(wrap(
        instance,
        ControlEvent::SetSawVol {
            amount: params.saw_vol,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetSquareVol {
            amount: params.square_vol,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetTriangleVol {
            amount: params.triangle_vol,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetSineVol {
            amount: params.sine_vol,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetPulse {
            width: params.pulse_width,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetSubVol {
            amount: params.sub_vol,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetSubOct {
            octaves: params.sub_octaves,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetCutoff {
            hz: params.cutoff_hz,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetResonance {
            amount: params.resonance,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetEnvelope {
            which: EnvelopeId::Amp,
            times: params.amp_env,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetFilterEnvAmount {
            amount: params.filter_env_amount,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetAssignableDest {
            dest: params.assignable_dest,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetAssignableAmount {
            amount: params.assignable_amount,
        },
    ));
    push(wrap(
        instance,
        ControlEvent::SetEnvVel {
            amount: params.env_vel,
        },
    ));
    push(MixerEvent::ToInstance {
        instance,
        event: InstanceEvent::SetVolume { amount: volume },
    });

    for which in [LfoId::One, LfoId::Two] {
        let lfo = params.lfos[which.index()];
        push(wrap(
            instance,
            ControlEvent::SetLfoDest {
                which,
                dest: lfo.dest,
            },
        ));
        push(wrap(
            instance,
            ControlEvent::SetLfoAmount {
                which,
                amount: lfo.amount,
            },
        ));
        push(wrap(
            instance,
            ControlEvent::SetLfoRate {
                which,
                rate_hz: lfo.rate_hz,
            },
        ));
        push(wrap(
            instance,
            ControlEvent::SetLfoWave {
                which,
                wave: lfo.wave,
            },
        ));
        push(wrap(
            instance,
            ControlEvent::SetLfoRetrig {
                which,
                on: lfo.retrigger,
            },
        ));
    }

    if params.env_link {
        push(wrap(instance, ControlEvent::SetEnvLink { on: true }));
    } else {
        push(wrap(instance, ControlEvent::SetEnvLink { on: false }));
        push(wrap(
            instance,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Filter,
                times: params.filter_env,
            },
        ));
        push(wrap(
            instance,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Assignable,
                times: params.assignable_env,
            },
        ));
    }

    PatchEvents { events, len }
}

fn wrap(instance: u8, event: ControlEvent) -> MixerEvent {
    MixerEvent::ToInstance {
        instance,
        event: InstanceEvent::Engine(event),
    }
}

fn log_uniform<R: Rng>(rng: &mut R, min: f32, max: f32) -> f32 {
    let log_min = libm::logf(min);
    let log_max = libm::logf(max);
    libm::expf(rng.gen_range(log_min..=log_max)).clamp(min, max)
}

fn random_adsr<R: Rng>(rng: &mut R) -> AdsrTimes {
    AdsrTimes {
        attack_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        decay_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        sustain: rng.gen_range(0.0..=1.0),
        release_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
    }
}

fn random_amount_for_dest<R: Rng>(rng: &mut R, dest: AssignableDest) -> f32 {
    match dest {
        AssignableDest::Resonance => rng.gen_range(RANDOM_RES_AMT_MIN..=RANDOM_RES_AMT_MAX),
        AssignableDest::PulseWidth => rng.gen_range(RANDOM_PW_AMT_MIN..=RANDOM_PW_AMT_MAX),
        AssignableDest::Amp => rng.gen_range(RANDOM_AMP_AMT_MIN..=RANDOM_AMP_AMT_MAX),
        AssignableDest::Off | AssignableDest::Pitch | AssignableDest::Cutoff => {
            rng.gen_range(RANDOM_AMT_MIN..=RANDOM_AMT_MAX)
        }
    }
}

fn random_lfo<R: Rng>(rng: &mut R) -> LfoParams {
    let dest = RANDOM_ASSIGNABLE_DESTS[rng.gen_range(0..RANDOM_ASSIGNABLE_DESTS.len())];
    LfoParams {
        dest,
        amount: random_amount_for_dest(rng, dest),
        rate_hz: log_uniform(rng, LFO_RATE_MIN_HZ, LFO_RATE_MAX_HZ),
        wave: RANDOM_LFO_WAVES[rng.gen_range(0..RANDOM_LFO_WAVES.len())],
        retrigger: rng.gen_bool(0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn random_patches_stay_in_range_and_respect_envlink() {
        for seed in 0..32 {
            let mut rng = StdRng::seed_from_u64(seed);
            let (params, volume) = random_patch(&mut rng);
            assert!(
                (RANDOM_VOL_MIN..=RANDOM_VOL_MAX).contains(&volume),
                "seed {seed}: volume {volume} out of range"
            );
            assert!((RANDOM_CUTOFF_MIN_HZ..=RANDOM_CUTOFF_MAX_HZ).contains(&params.cutoff_hz));
            assert!((0.0..=RANDOM_RES_MAX).contains(&params.resonance));
            assert!((RANDOM_PULSE_MIN..=RANDOM_PULSE_MAX).contains(&params.pulse_width));

            let events = patch_events(2, &params, volume);
            let mut saw_link_on = false;
            let mut saw_link_off = false;
            let mut extra_env_times = 0usize;
            let mut saw_volume = false;
            let mut saw_osc_levels = 0usize;

            for event in events.as_slice() {
                match event {
                    MixerEvent::ToInstance { instance, event } => {
                        assert_eq!(*instance, 2);
                        match event {
                            InstanceEvent::SetEnabled { .. }
                            | InstanceEvent::SetListenChannel { .. } => {
                                panic!(
                                    "seed {seed}: random must not change enabled or listen channel"
                                );
                            }
                            InstanceEvent::SetVolume { amount } => {
                                assert!((RANDOM_VOL_MIN..=RANDOM_VOL_MAX).contains(amount));
                                saw_volume = true;
                            }
                            InstanceEvent::Engine(control) => match control {
                                ControlEvent::SetSawVol { .. }
                                | ControlEvent::SetSquareVol { .. }
                                | ControlEvent::SetTriangleVol { .. }
                                | ControlEvent::SetSineVol { .. } => saw_osc_levels += 1,
                                ControlEvent::SetEnvLink { on: true } => saw_link_on = true,
                                ControlEvent::SetEnvLink { on: false } => saw_link_off = true,
                                ControlEvent::SetEnvelope { which, .. } => match which {
                                    EnvelopeId::Amp => {}
                                    EnvelopeId::Filter | EnvelopeId::Assignable => {
                                        extra_env_times += 1;
                                    }
                                },
                                _ => {}
                            },
                        }
                    }
                    MixerEvent::MidiNoteOn { .. } | MixerEvent::MidiNoteOff { .. } => {
                        panic!("seed {seed}: random must not emit MIDI events");
                    }
                }
            }

            assert!(saw_volume, "seed {seed}: random must include volume");
            assert_eq!(saw_osc_levels, 4, "seed {seed}: four at-pitch osc levels");
            assert!(
                saw_link_on ^ saw_link_off,
                "seed {seed}: expected exactly one envlink setting"
            );
            if saw_link_on {
                assert_eq!(extra_env_times, 0);
            } else {
                assert_eq!(extra_env_times, 2);
            }
        }
    }
}
