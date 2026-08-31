//! Shared laptop-host plumbing for `host-wsl` and `host-windows`.

mod commands;

use commands::{CommandOutcome, CommandSession};

use std::collections::HashSet;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SizedSample};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use engine::{Mixer, MixerEvent};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use rtrb::{Producer, RingBuffer};

const EVENT_QUEUE_CAPACITY: usize = 128;
/// MIDI note for C4; letter-key map builds one octave up from here.
const KEYBOARD_ROOT_NOTE: u8 = 60;

/// Opens audio and MIDI (or keyboard fallback) and runs until the user quits.
pub fn run(midi_client_name: &str) -> Result<(), Box<dyn Error>> {
    let cpal_host = cpal::default_host();
    let device = select_output_device(&cpal_host)?;
    let supported_config = device.default_output_config()?;

    println!("Output device: {device}");
    println!(
        "Sample format: {:?}, sample rate: {} Hz, channels: {}",
        supported_config.sample_format(),
        supported_config.sample_rate(),
        supported_config.channels()
    );

    let (producer, consumer) = RingBuffer::<MixerEvent>::new(EVENT_QUEUE_CAPACITY);
    // Mutex is only for MIDI callback vs terminal (never held on the audio path).
    let producer = Arc::new(Mutex::new(producer));
    let stream_config = supported_config.config();
    let mut session = CommandSession::new();

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, stream_config, consumer)?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, stream_config, consumer)?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, stream_config, consumer)?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;

    // Keep the MIDI connection alive for the whole run when a port exists.
    if let Some(_midi_connection) = try_open_midi_input(midi_client_name, Arc::clone(&producer))? {
        println!("Type engine commands (cutoff, res, attack, …) or q then Enter to quit.");
        commands::print_help();
        run_line_command_loop(&producer, &mut session)?;
    } else {
        println!("Using laptop keyboard.");
        print_keyboard_map();
        commands::print_help();
        println!("Press / for a param command, q to quit.");
        run_keyboard_loop(&producer, &mut session)?;
    }

    Ok(())
}

fn select_output_device(cpal_host: &cpal::Host) -> Result<Device, Box<dyn Error>> {
    let devices: Vec<Device> = cpal_host.output_devices()?.collect();
    if devices.is_empty() {
        return Err("no output devices found".into());
    }

    if devices.len() == 1 {
        return Ok(devices.into_iter().next().expect("len checked"));
    }

    println!("Audio output devices:");
    for (index, device) in devices.iter().enumerate() {
        // Device's Display uses the device name when available.
        println!("  {index}: {device}");
    }

    let index = prompt_index("Select audio output number", devices.len())?;
    Ok(devices.into_iter().nth(index).expect("index checked"))
}

fn prompt_index(prompt: &str, count: usize) -> Result<usize, Box<dyn Error>> {
    loop {
        print!("{prompt} (0-{}): ", count - 1);
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        match trimmed.parse::<usize>() {
            Ok(index) if index < count => return Ok(index),
            _ => println!("Enter a number between 0 and {}.", count - 1),
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: rtrb::Consumer<MixerEvent>,
) -> Result<cpal::Stream, Box<dyn Error>>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut mixer = Mixer::new(config.sample_rate as f32);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
            while let Ok(event) = consumer.pop() {
                mixer.apply(event);
            }

            for frame in data.chunks_mut(channels) {
                let sample = mixer.next_sample();
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

fn push_event(producer: &Arc<Mutex<Producer<MixerEvent>>>, event: MixerEvent) {
    // If the audio thread is behind, drop the event rather than blocking the audio path.
    // Contending briefly with the other control writer is fine; we never lock in the callback.
    if let Ok(mut guard) = producer.lock() {
        let _ = guard.push(event);
    }
}

fn try_open_midi_input(
    client_name: &str,
    producer: Arc<Mutex<Producer<MixerEvent>>>,
) -> Result<Option<MidiInputConnection<()>>, Box<dyn Error>> {
    // On some setups (e.g. WSL without an ALSA sequencer) midir cannot initialize.
    // Treat that like "no ports" so the keyboard fallback still works.
    let midi_in = match MidiInput::new(client_name) {
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

    let port = select_midi_port(&midi_in, &ports)?;
    let port_name = midi_in.port_name(&port)?;
    println!("MIDI input: {port_name}");

    match midi_in.connect(
        &port,
        "stone-raft-input",
        move |_stamp, message, _| {
            if let Some(event) = parse_midi_message(message) {
                push_event(&producer, event);
            }
        },
        (),
    ) {
        Ok(connection) => Ok(Some(connection)),
        Err(err) => Err(format!("MIDI connect failed: {err}").into()),
    }
}

fn select_midi_port(
    midi_in: &MidiInput,
    ports: &[MidiInputPort],
) -> Result<MidiInputPort, Box<dyn Error>> {
    if ports.len() == 1 {
        return Ok(ports[0].clone());
    }

    println!("MIDI input ports:");
    for (index, port) in ports.iter().enumerate() {
        let name = midi_in
            .port_name(port)
            .unwrap_or_else(|_| "<unknown>".to_string());
        println!("  {index}: {name}");
    }

    let index = prompt_index("Select MIDI input number", ports.len())?;
    Ok(ports[index].clone())
}

fn parse_midi_message(message: &[u8]) -> Option<MixerEvent> {
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

fn run_keyboard_loop(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut CommandSession,
) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let result = keyboard_event_loop(producer, session);
    disable_raw_mode()?;
    // Move to a new line after raw mode so the shell prompt is clean.
    println!();
    result
}

fn keyboard_event_loop(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut CommandSession,
) -> Result<(), Box<dyn Error>> {
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
                    code: KeyCode::Char('/'),
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    // Leave raw mode so the user can type a normal line command.
                    disable_raw_mode()?;
                    println!();
                    print!("command> ");
                    io::stdout().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    dispatch_param_line(producer, session, line.trim(), Some("(empty command)"));
                    enable_raw_mode()?;
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if let Some(note) = key_to_note(code) {
                        if pressed.insert(note) {
                            if session.current_enabled() {
                                push_event(producer, session.keyboard_note_event(note, true));
                            } else {
                                print!(
                                    "\r\nengine {} is off; type: on\r\n",
                                    session.current_engine_number()
                                );
                                io::stdout().flush()?;
                            }
                        }
                    }
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Release,
                    ..
                }) => {
                    if let Some(note) = key_to_note(code) {
                        if pressed.remove(&note) && session.current_enabled() {
                            push_event(producer, session.keyboard_note_event(note, false));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Line mode when MIDI is connected: param commands or q to quit.
fn run_line_command_loop(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut CommandSession,
) -> Result<(), Box<dyn Error>> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("q") {
            break;
        }
        dispatch_param_line(producer, session, trimmed, None);
    }
    Ok(())
}

fn dispatch_param_line(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut CommandSession,
    line: &str,
    empty_message: Option<&str>,
) {
    match session.handle(line) {
        CommandOutcome::Applied { events, text } => {
            for event in events {
                push_event(producer, event);
            }
            println!("ok");
            print!("{text}");
        }
        CommandOutcome::Empty => {
            if let Some(message) = empty_message {
                println!("{message}");
            }
        }
        CommandOutcome::Error(err) => println!("error: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_parse_keeps_channel_5() {
        match parse_midi_message(&[0x94, 60, 100]) {
            Some(MixerEvent::MidiNoteOn {
                channel: 5,
                note: 60,
                velocity: 100,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
