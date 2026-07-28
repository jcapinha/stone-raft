use std::collections::HashSet;
use std::error::Error;
use std::io::{self, BufRead};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use engine::Engine;
use midir::{MidiInput, MidiInputConnection};
use rtrb::{Producer, RingBuffer};

const EVENT_QUEUE_CAPACITY: usize = 128;
const KEYBOARD_VELOCITY: u8 = 100;
/// MIDI note for C4; letter-key map builds one octave up from here.
const KEYBOARD_ROOT_NOTE: u8 = 60;

#[derive(Debug, Clone, Copy)]
enum NoteEvent {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
}

fn main() -> Result<(), Box<dyn Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default output device found")?;
    let supported_config = device.default_output_config()?;

    println!("Output device: {device}");
    println!(
        "Sample format: {:?}, sample rate: {} Hz, channels: {}",
        supported_config.sample_format(),
        supported_config.sample_rate(),
        supported_config.channels()
    );

    let (producer, consumer) = RingBuffer::<NoteEvent>::new(EVENT_QUEUE_CAPACITY);
    let mut producer = Some(producer);
    let stream_config = supported_config.config();

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, stream_config, consumer)?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, stream_config, consumer)?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, stream_config, consumer)?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;

    // Keep the MIDI connection alive for the whole run when a port exists.
    if let Some(_midi_connection) = try_open_first_midi_input(&mut producer)? {
        println!("Type q then Enter to quit.");
        run_quit_only_loop()?;
    } else {
        let producer = producer
            .as_mut()
            .ok_or("keyboard event producer missing")?;
        println!("Using laptop keyboard.");
        print_keyboard_map();
        println!("Press q to quit.");
        run_keyboard_loop(producer)?;
    }

    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: rtrb::Consumer<NoteEvent>,
) -> Result<cpal::Stream, Box<dyn Error>>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut engine = Engine::new(config.sample_rate as f32);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
            while let Ok(event) = consumer.pop() {
                match event {
                    NoteEvent::NoteOn { note, velocity } => engine.note_on(note, velocity),
                    NoteEvent::NoteOff { note } => engine.note_off(note),
                }
            }

            for frame in data.chunks_mut(channels) {
                let sample = engine.next_sample();
                let value = T::from_sample(sample);
                for out in frame.iter_mut() {
                    *out = value;
                }
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    Ok(stream)
}

fn push_event(producer: &mut Producer<NoteEvent>, event: NoteEvent) {
    // If the audio thread is behind, drop the event rather than blocking.
    let _ = producer.push(event);
}

fn try_open_first_midi_input(
    producer: &mut Option<Producer<NoteEvent>>,
) -> Result<Option<MidiInputConnection<()>>, Box<dyn Error>> {
    // On some setups (e.g. WSL without an ALSA sequencer) midir cannot initialize.
    // Treat that like "no ports" so the keyboard fallback still works.
    let midi_in = match MidiInput::new("stone-raft host-laptop") {
        Ok(midi_in) => midi_in,
        Err(err) => {
            eprintln!("MIDI unavailable ({err}); falling back to keyboard.");
            return Ok(None);
        }
    };
    let ports = midi_in.ports();
    if ports.is_empty() {
        return Ok(None);
    }

    let port = &ports[0];
    let port_name = midi_in.port_name(port)?;
    println!("MIDI input: {port_name}");

    let mut midi_producer = producer
        .take()
        .ok_or("MIDI event producer already taken")?;

    match midi_in.connect(
        port,
        "stone-raft-input",
        move |_stamp, message, _| {
            if let Some(event) = parse_midi_message(message) {
                push_event(&mut midi_producer, event);
            }
        },
        (),
    ) {
        Ok(connection) => Ok(Some(connection)),
        Err(err) => {
            // Producer was moved into the failed connect; cannot recover the keyboard path.
            Err(format!("MIDI connect failed: {err}").into())
        }
    }
}

fn parse_midi_message(message: &[u8]) -> Option<NoteEvent> {
    if message.len() < 2 {
        return None;
    }

    let status = message[0];
    let note = message[1];
    let velocity = message.get(2).copied().unwrap_or(0);
    let kind = status & 0xF0;

    match kind {
        0x90 if velocity > 0 => Some(NoteEvent::NoteOn { note, velocity }),
        0x90 | 0x80 => Some(NoteEvent::NoteOff { note }),
        _ => None,
    }
}

fn print_keyboard_map() {
    println!("Keyboard map (C4 octave):");
    println!("  A W S E D F T G Y H U J K");
    println!("  C C# D D# E F F# G G# A A# B C");
}

fn key_to_note(code: KeyCode) -> Option<u8> {
    let offset = match code {
        KeyCode::Char('a') | KeyCode::Char('A') => 0,
        KeyCode::Char('w') | KeyCode::Char('W') => 1,
        KeyCode::Char('s') | KeyCode::Char('S') => 2,
        KeyCode::Char('e') | KeyCode::Char('E') => 3,
        KeyCode::Char('d') | KeyCode::Char('D') => 4,
        KeyCode::Char('f') | KeyCode::Char('F') => 5,
        KeyCode::Char('t') | KeyCode::Char('T') => 6,
        KeyCode::Char('g') | KeyCode::Char('G') => 7,
        KeyCode::Char('y') | KeyCode::Char('Y') => 8,
        KeyCode::Char('h') | KeyCode::Char('H') => 9,
        KeyCode::Char('u') | KeyCode::Char('U') => 10,
        KeyCode::Char('j') | KeyCode::Char('J') => 11,
        KeyCode::Char('k') | KeyCode::Char('K') => 12,
        _ => return None,
    };
    Some(KEYBOARD_ROOT_NOTE + offset)
}

fn run_keyboard_loop(producer: &mut Producer<NoteEvent>) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let result = keyboard_event_loop(producer);
    disable_raw_mode()?;
    // Move to a new line after raw mode so the shell prompt is clean.
    println!();
    result
}

fn keyboard_event_loop(producer: &mut Producer<NoteEvent>) -> Result<(), Box<dyn Error>> {
    let mut pressed: HashSet<u8> = HashSet::new();

    loop {
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('q') | KeyCode::Char('Q'),
                    kind: KeyEventKind::Press,
                    ..
                }) => break,
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if let Some(note) = key_to_note(code) {
                        if pressed.insert(note) {
                            push_event(
                                producer,
                                NoteEvent::NoteOn {
                                    note,
                                    velocity: KEYBOARD_VELOCITY,
                                },
                            );
                        }
                    }
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Release,
                    ..
                }) => {
                    if let Some(note) = key_to_note(code) {
                        if pressed.remove(&note) {
                            push_event(producer, NoteEvent::NoteOff { note });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// When MIDI is connected, block until the user types q (line mode is enough).
fn run_quit_only_loop() -> Result<(), Box<dyn Error>> {
    for line in io::stdin().lock().lines() {
        if line?.trim().eq_ignore_ascii_case("q") {
            break;
        }
    }
    Ok(())
}
