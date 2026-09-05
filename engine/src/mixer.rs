use crate::{ControlEvent, Engine};

pub const ENGINE_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MixerEvent {
    /// `instance` is 1-based (`eng 1` through `eng 4`).
    ToInstance {
        instance: u8,
        event: InstanceEvent,
    },
    MidiNoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    MidiNoteOff {
        channel: u8,
        note: u8,
    },
}

impl MixerEvent {
    /// Parses a MIDI channel-voice message into a mixer event.
    ///
    /// Channel nibble 0..15 becomes listen channel 1..16.
    /// Note-on with velocity 0 is treated as note-off.
    /// Non-note messages and truncated buffers return `None`.
    pub fn from_midi_bytes(message: &[u8]) -> Option<Self> {
        if message.len() < 2 {
            return None;
        }

        let status = message[0];
        let note = message[1];
        let velocity = message.get(2).copied().unwrap_or(0);
        let kind = status & 0xF0;
        let channel = (status & 0x0F) + 1;

        match kind {
            0x90 if velocity > 0 => Some(MixerEvent::MidiNoteOn {
                channel,
                note,
                velocity,
            }),
            0x90 | 0x80 => Some(MixerEvent::MidiNoteOff { channel, note }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InstanceEvent {
    Engine(ControlEvent),
    SetEnabled { on: bool },
    SetListenChannel { channel: u8 },
    SetVolume { amount: f32 },
}

/// Maps a 1-based instance number to a 0-based array index.
fn instance_index(instance: u8) -> Option<usize> {
    let index = (instance as usize).wrapping_sub(1);
    (index < ENGINE_COUNT).then_some(index)
}

struct MixerInstance {
    engine: Engine,
    enabled: bool,
    listen_channel: u8,
    volume: f32,
}

impl MixerInstance {
    fn new(sample_rate_hz: f32, enabled: bool, listen_channel: u8, volume: f32) -> Self {
        Self {
            engine: Engine::new(sample_rate_hz),
            enabled,
            listen_channel,
            volume,
        }
    }
}

/// Four engine instances mixed to one mono output. Disabled instances skip DSP.
pub struct Mixer {
    instances: [MixerInstance; ENGINE_COUNT],
}

impl Mixer {
    pub fn new(sample_rate_hz: f32) -> Self {
        Self {
            instances: [
                MixerInstance::new(sample_rate_hz, true, 1, 1.0),
                MixerInstance::new(sample_rate_hz, false, 2, 1.0),
                MixerInstance::new(sample_rate_hz, false, 3, 1.0),
                MixerInstance::new(sample_rate_hz, false, 4, 1.0),
            ],
        }
    }

    pub fn apply(&mut self, event: MixerEvent) {
        match event {
            MixerEvent::ToInstance { instance, event } => {
                let Some(index) = instance_index(instance) else {
                    return;
                };
                self.apply_instance(index, event);
            }
            MixerEvent::MidiNoteOn {
                channel,
                note,
                velocity,
            } => {
                for instance in self.instances.iter_mut() {
                    if instance.enabled && instance.listen_channel == channel {
                        instance
                            .engine
                            .apply(ControlEvent::NoteOn { note, velocity });
                    }
                }
            }
            MixerEvent::MidiNoteOff { channel, note } => {
                for instance in self.instances.iter_mut() {
                    if instance.enabled && instance.listen_channel == channel {
                        instance.engine.apply(ControlEvent::NoteOff { note });
                    }
                }
            }
        }
    }

    fn apply_instance(&mut self, index: usize, event: InstanceEvent) {
        let instance = &mut self.instances[index];
        match event {
            InstanceEvent::Engine(control) => instance.engine.apply(control),
            InstanceEvent::SetEnabled { on } => {
                instance.enabled = on;
                if !on {
                    instance.engine.silence();
                }
            }
            InstanceEvent::SetListenChannel { channel } => {
                instance.listen_channel = channel.clamp(1, 16);
            }
            InstanceEvent::SetVolume { amount } => {
                instance.volume = amount.clamp(0.0, 1.0);
            }
        }
    }

    /// Mixes enabled instances only. Disabled instances do not run engine DSP.
    pub fn next_sample(&mut self) -> f32 {
        let mut mix = 0.0;
        for instance in self.instances.iter_mut() {
            if !instance.enabled {
                continue;
            }
            mix += instance.engine.next_sample() * instance.volume;
        }
        mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdsrTimes, EnvelopeId};

    const SAMPLE_RATE_HZ: f32 = 48_000.0;
    const NOTE: u8 = 60;
    const VELOCITY: u8 = 127;

    fn fast_amp(instance: u8) -> MixerEvent {
        MixerEvent::ToInstance {
            instance,
            event: InstanceEvent::Engine(ControlEvent::SetEnvelope {
                which: EnvelopeId::Amp,
                times: AdsrTimes {
                    attack_ms: 1.0,
                    decay_ms: 1.0,
                    sustain: 1.0,
                    release_ms: 1.0,
                },
            }),
        }
    }

    fn open_cutoff(instance: u8) -> MixerEvent {
        MixerEvent::ToInstance {
            instance,
            event: InstanceEvent::Engine(ControlEvent::SetCutoff { hz: 8_000.0 }),
        }
    }

    fn enable(instance: u8, on: bool) -> MixerEvent {
        MixerEvent::ToInstance {
            instance,
            event: InstanceEvent::SetEnabled { on },
        }
    }

    fn listen(instance: u8, channel: u8) -> MixerEvent {
        MixerEvent::ToInstance {
            instance,
            event: InstanceEvent::SetListenChannel { channel },
        }
    }

    fn volume(instance: u8, amount: f32) -> MixerEvent {
        MixerEvent::ToInstance {
            instance,
            event: InstanceEvent::SetVolume { amount },
        }
    }

    fn midi_on(channel: u8) -> MixerEvent {
        MixerEvent::MidiNoteOn {
            channel,
            note: NOTE,
            velocity: VELOCITY,
        }
    }

    fn peak_after(mixer: &mut Mixer, skip: usize, count: usize) -> f32 {
        for _ in 0..skip {
            mixer.next_sample();
        }
        (0..count)
            .map(|_| mixer.next_sample().abs())
            .fold(0.0f32, f32::max)
    }

    fn prepare_instance(mixer: &mut Mixer, instance: u8) {
        mixer.apply(fast_amp(instance));
        mixer.apply(open_cutoff(instance));
    }

    #[test]
    fn instance_1_default_sounds_on_channel_1_not_channel_2() {
        let mut on_ch1 = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut on_ch1, 1);
        on_ch1.apply(midi_on(1));
        let peak_ch1 = peak_after(&mut on_ch1, 2_000, 1_000);
        assert!(
            peak_ch1 > 0.01,
            "channel 1 should sound on instance 1, peak={peak_ch1}"
        );

        let mut on_ch2 = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut on_ch2, 1);
        on_ch2.apply(midi_on(2));
        let peak_ch2 = peak_after(&mut on_ch2, 2_000, 1_000);
        assert!(
            peak_ch2 < 1e-4,
            "channel 2 should be silence at startup, peak={peak_ch2}"
        );
    }

    #[test]
    fn instance_2_off_ignores_midi_until_enabled() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut mixer, 2);
        mixer.apply(midi_on(2));
        let off_peak = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            off_peak < 1e-4,
            "disabled instance 2 should ignore channel 2, peak={off_peak}"
        );

        mixer.apply(enable(2, true));
        mixer.apply(midi_on(2));
        let on_peak = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            on_peak > 0.01,
            "enabled instance 2 should sound on channel 2, peak={on_peak}"
        );
    }

    #[test]
    fn instance_2_listen_channel_5_ignores_channel_2() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut mixer, 2);
        mixer.apply(enable(2, true));
        mixer.apply(listen(2, 5));

        mixer.apply(midi_on(5));
        let peak_ch5 = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            peak_ch5 > 0.01,
            "channel 5 should sound after ch 5, peak={peak_ch5}"
        );

        let mut other = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut other, 2);
        other.apply(enable(2, true));
        other.apply(listen(2, 5));
        other.apply(midi_on(2));
        let peak_ch2 = peak_after(&mut other, 2_000, 1_000);
        assert!(
            peak_ch2 < 1e-4,
            "channel 2 should not sound after ch 5, peak={peak_ch2}"
        );
    }

    #[test]
    fn two_instances_on_same_channel_mix_louder_than_one() {
        let mut one = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut one, 1);
        one.apply(listen(1, 5));
        one.apply(midi_on(5));
        let peak_one = peak_after(&mut one, 2_000, 1_000);

        let mut two = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut two, 1);
        prepare_instance(&mut two, 2);
        two.apply(listen(1, 5));
        two.apply(enable(2, true));
        two.apply(listen(2, 5));
        two.apply(midi_on(5));
        let peak_two = peak_after(&mut two, 2_000, 1_000);

        assert!(
            peak_two > peak_one * 1.4,
            "two instances should mix louder; one={peak_one} two={peak_two}"
        );
    }

    #[test]
    fn instance_volume_scales_output_proportionally() {
        let mut full = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut full, 1);
        full.apply(midi_on(1));
        let full_peak = peak_after(&mut full, 2_000, 1_000);

        let mut half = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut half, 1);
        half.apply(volume(1, 0.5));
        half.apply(midi_on(1));
        let half_peak = peak_after(&mut half, 2_000, 1_000);

        let ratio = half_peak / full_peak;
        assert!(
            (ratio - 0.5).abs() < 0.01,
            "vol 0.5 should produce half the peak of vol 1; full={full_peak} half={half_peak}"
        );
    }

    #[test]
    fn balanced_four_instance_mix_stays_within_output_range() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        for instance in 1..=4 {
            prepare_instance(&mut mixer, instance);
            mixer.apply(enable(instance, true));
            mixer.apply(listen(instance, 1));
            mixer.apply(volume(instance, 0.25));
        }

        mixer.apply(midi_on(1));
        let peak = peak_after(&mut mixer, 2_000, 1_000);

        assert!(
            peak <= 1.0,
            "balanced engine volumes should keep the combined mix in range; peak={peak}"
        );
    }

    #[test]
    fn off_after_held_note_silences_immediately() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_instance(&mut mixer, 1);
        mixer.apply(midi_on(1));
        let held = peak_after(&mut mixer, 2_000, 200);
        assert!(held > 0.01, "held note should sound, peak={held}");

        mixer.apply(enable(1, false));
        let muted = peak_after(&mut mixer, 0, 200);
        assert!(muted < 1e-4, "off should silence immediately, peak={muted}");
    }

    #[test]
    fn disabled_instance_does_not_run_next_sample() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        let before = mixer.instances[1].engine.next_sample_call_count();
        for _ in 0..1_000 {
            mixer.next_sample();
        }
        assert_eq!(
            mixer.instances[1].engine.next_sample_call_count(),
            before,
            "disabled instance 2 must not run Engine::next_sample"
        );
        assert!(
            mixer.instances[0].engine.next_sample_call_count() >= 1_000,
            "enabled instance 1 should run DSP"
        );
    }

    #[test]
    fn out_of_range_instance_is_ignored() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        mixer.apply(MixerEvent::ToInstance {
            instance: 0,
            event: InstanceEvent::SetEnabled { on: false },
        });
        mixer.apply(MixerEvent::ToInstance {
            instance: 5,
            event: InstanceEvent::SetEnabled { on: true },
        });
        assert!(mixer.instances[0].enabled);
        assert!(!mixer.instances[1].enabled);
    }

    #[test]
    fn from_midi_bytes_keeps_channel_5() {
        match MixerEvent::from_midi_bytes(&[0x94, 60, 100]) {
            Some(MixerEvent::MidiNoteOn {
                channel: 5,
                note: 60,
                velocity: 100,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_midi_bytes_note_on_velocity_zero_is_note_off() {
        match MixerEvent::from_midi_bytes(&[0x94, 60, 0]) {
            Some(MixerEvent::MidiNoteOff {
                channel: 5,
                note: 60,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_midi_bytes_note_off_status() {
        match MixerEvent::from_midi_bytes(&[0x85, 61, 0]) {
            Some(MixerEvent::MidiNoteOff {
                channel: 6,
                note: 61,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn from_midi_bytes_ignores_short_and_non_note() {
        assert_eq!(MixerEvent::from_midi_bytes(&[0x94]), None);
        assert_eq!(MixerEvent::from_midi_bytes(&[]), None);
        assert_eq!(MixerEvent::from_midi_bytes(&[0xB0, 7, 100]), None);
    }
}
