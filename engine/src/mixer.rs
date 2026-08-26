use crate::{ControlEvent, Engine};

pub const ENGINE_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MixerEvent {
    ToSlot { slot: u8, event: SlotEvent },
    MidiNoteOn { channel: u8, note: u8, velocity: u8 },
    MidiNoteOff { channel: u8, note: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotEvent {
    Engine(ControlEvent),
    SetEnabled { on: bool },
    SetListenChannel { channel: u8 },
    SetVolume { amount: f32 },
}

struct MixerSlot {
    engine: Engine,
    enabled: bool,
    listen_channel: u8,
    volume: f32,
}

impl MixerSlot {
    fn new(sample_rate_hz: f32, enabled: bool, listen_channel: u8, volume: f32) -> Self {
        Self {
            engine: Engine::new(sample_rate_hz),
            enabled,
            listen_channel,
            volume,
        }
    }
}

/// Four engine instances mixed to one mono output. Disabled slots skip DSP.
pub struct Mixer {
    slots: [MixerSlot; ENGINE_COUNT],
}

impl Mixer {
    pub fn new(sample_rate_hz: f32) -> Self {
        Self {
            slots: [
                MixerSlot::new(sample_rate_hz, true, 1, 1.0),
                MixerSlot::new(sample_rate_hz, false, 2, 1.0),
                MixerSlot::new(sample_rate_hz, false, 3, 1.0),
                MixerSlot::new(sample_rate_hz, false, 4, 1.0),
            ],
        }
    }

    pub fn apply(&mut self, event: MixerEvent) {
        match event {
            MixerEvent::ToSlot { slot, event } => {
                let index = slot as usize;
                if index >= ENGINE_COUNT {
                    return;
                }
                self.apply_slot(index, event);
            }
            MixerEvent::MidiNoteOn {
                channel,
                note,
                velocity,
            } => {
                for slot in self.slots.iter_mut() {
                    if slot.enabled && slot.listen_channel == channel {
                        slot.engine.apply(ControlEvent::NoteOn { note, velocity });
                    }
                }
            }
            MixerEvent::MidiNoteOff { channel, note } => {
                for slot in self.slots.iter_mut() {
                    if slot.enabled && slot.listen_channel == channel {
                        slot.engine.apply(ControlEvent::NoteOff { note });
                    }
                }
            }
        }
    }

    fn apply_slot(&mut self, index: usize, event: SlotEvent) {
        let slot = &mut self.slots[index];
        match event {
            SlotEvent::Engine(control) => slot.engine.apply(control),
            SlotEvent::SetEnabled { on } => {
                slot.enabled = on;
                if !on {
                    slot.engine.silence();
                }
            }
            SlotEvent::SetListenChannel { channel } => {
                slot.listen_channel = channel.clamp(1, 16);
            }
            SlotEvent::SetVolume { amount } => {
                slot.volume = amount.clamp(0.0, 1.0);
            }
        }
    }

    /// Mixes enabled slots only. Disabled slots do not run engine DSP.
    pub fn next_sample(&mut self) -> f32 {
        let mut mix = 0.0;
        for slot in self.slots.iter_mut() {
            if !slot.enabled {
                continue;
            }
            mix += slot.engine.next_sample() * slot.volume;
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

    fn fast_amp(slot: u8) -> MixerEvent {
        MixerEvent::ToSlot {
            slot,
            event: SlotEvent::Engine(ControlEvent::SetEnvelope {
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

    fn open_cutoff(slot: u8) -> MixerEvent {
        MixerEvent::ToSlot {
            slot,
            event: SlotEvent::Engine(ControlEvent::SetCutoff { hz: 8_000.0 }),
        }
    }

    fn enable(slot: u8, on: bool) -> MixerEvent {
        MixerEvent::ToSlot {
            slot,
            event: SlotEvent::SetEnabled { on },
        }
    }

    fn listen(slot: u8, channel: u8) -> MixerEvent {
        MixerEvent::ToSlot {
            slot,
            event: SlotEvent::SetListenChannel { channel },
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

    fn prepare_slot(mixer: &mut Mixer, slot: u8) {
        mixer.apply(fast_amp(slot));
        mixer.apply(open_cutoff(slot));
    }

    #[test]
    fn slot_1_default_sounds_on_channel_1_not_channel_2() {
        let mut on_ch1 = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut on_ch1, 0);
        on_ch1.apply(midi_on(1));
        let peak_ch1 = peak_after(&mut on_ch1, 2_000, 1_000);
        assert!(
            peak_ch1 > 0.01,
            "channel 1 should sound on slot 1, peak={peak_ch1}"
        );

        let mut on_ch2 = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut on_ch2, 0);
        on_ch2.apply(midi_on(2));
        let peak_ch2 = peak_after(&mut on_ch2, 2_000, 1_000);
        assert!(
            peak_ch2 < 1e-4,
            "channel 2 should be silence at startup, peak={peak_ch2}"
        );
    }

    #[test]
    fn slot_2_off_ignores_midi_until_enabled() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut mixer, 1);
        mixer.apply(midi_on(2));
        let off_peak = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            off_peak < 1e-4,
            "disabled slot 2 should ignore channel 2, peak={off_peak}"
        );

        mixer.apply(enable(1, true));
        mixer.apply(midi_on(2));
        let on_peak = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            on_peak > 0.01,
            "enabled slot 2 should sound on channel 2, peak={on_peak}"
        );
    }

    #[test]
    fn slot_2_listen_channel_5_ignores_channel_2() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut mixer, 1);
        mixer.apply(enable(1, true));
        mixer.apply(listen(1, 5));

        mixer.apply(midi_on(5));
        let peak_ch5 = peak_after(&mut mixer, 2_000, 1_000);
        assert!(
            peak_ch5 > 0.01,
            "channel 5 should sound after ch 5, peak={peak_ch5}"
        );

        let mut other = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut other, 1);
        other.apply(enable(1, true));
        other.apply(listen(1, 5));
        other.apply(midi_on(2));
        let peak_ch2 = peak_after(&mut other, 2_000, 1_000);
        assert!(
            peak_ch2 < 1e-4,
            "channel 2 should not sound after ch 5, peak={peak_ch2}"
        );
    }

    #[test]
    fn two_slots_on_same_channel_mix_louder_than_one() {
        let mut one = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut one, 0);
        one.apply(listen(0, 5));
        one.apply(midi_on(5));
        let peak_one = peak_after(&mut one, 2_000, 1_000);

        let mut two = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut two, 0);
        prepare_slot(&mut two, 1);
        two.apply(listen(0, 5));
        two.apply(enable(1, true));
        two.apply(listen(1, 5));
        two.apply(midi_on(5));
        let peak_two = peak_after(&mut two, 2_000, 1_000);

        assert!(
            peak_two > peak_one * 1.4,
            "two slots should mix louder; one={peak_one} two={peak_two}"
        );
    }

    #[test]
    fn off_after_held_note_silences_immediately() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        prepare_slot(&mut mixer, 0);
        mixer.apply(midi_on(1));
        let held = peak_after(&mut mixer, 2_000, 200);
        assert!(held > 0.01, "held note should sound, peak={held}");

        mixer.apply(enable(0, false));
        let muted = peak_after(&mut mixer, 0, 200);
        assert!(muted < 1e-4, "off should silence immediately, peak={muted}");
    }

    #[test]
    fn disabled_slot_does_not_run_next_sample() {
        let mut mixer = Mixer::new(SAMPLE_RATE_HZ);
        let before = mixer.slots[1].engine.next_sample_call_count();
        for _ in 0..1_000 {
            mixer.next_sample();
        }
        assert_eq!(
            mixer.slots[1].engine.next_sample_call_count(),
            before,
            "disabled slot 2 must not run Engine::next_sample"
        );
        assert!(
            mixer.slots[0].engine.next_sample_call_count() >= 1_000,
            "enabled slot 1 should run DSP"
        );
    }
}
