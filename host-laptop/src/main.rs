use std::collections::HashSet;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use engine::{Engine, Waveform};
use midir::{MidiInput, MidiInputConnection};
use rtrb::{Producer, RingBuffer};

const EVENT_QUEUE_CAPACITY: usize = 128;
const KEYBOARD_VELOCITY: u8 = 100;
/// MIDI note for C4; letter-key map builds one octave up from here.
const KEYBOARD_ROOT_NOTE: u8 = 60;

#[derive(Debug, Clone, Copy)]
enum ControlEvent {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    SetCutoff { hz: f32 },
    SetResonance { amount: f32 },
    SetAttack { ms: f32 },
    SetDecay { ms: f32 },
    SetSustain { level: f32 },
    SetRelease { ms: f32 },
    SetWave { waveform: Waveform },
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

    let (producer, consumer) = RingBuffer::<ControlEvent>::new(EVENT_QUEUE_CAPACITY);
    // Mutex is only for MIDI callback vs terminal (never held on the audio path).
    let producer = Arc::new(Mutex::new(producer));
    let stream_config = supported_config.config();

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, stream_config, consumer)?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, stream_config, consumer)?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, stream_config, consumer)?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;

    // Keep the MIDI connection alive for the whole run when a port exists.
    if let Some(_midi_connection) = try_open_first_midi_input(Arc::clone(&producer))? {
        println!("Type engine commands (cutoff, res, attack, …) or q then Enter to quit.");
        print_param_help();
        run_line_command_loop(&producer)?;
    } else {
        println!("Using laptop keyboard.");
        print_keyboard_map();
        print_param_help();
        println!("Press / for a param command, q to quit.");
        run_keyboard_loop(&producer)?;
    }

    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: rtrb::Consumer<ControlEvent>,
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
                apply_event(&mut engine, event);
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

fn apply_event(engine: &mut Engine, event: ControlEvent) {
    match event {
        ControlEvent::NoteOn { note, velocity } => engine.note_on(note, velocity),
        ControlEvent::NoteOff { note } => engine.note_off(note),
        ControlEvent::SetCutoff { hz } => engine.set_cutoff_hz(hz),
        ControlEvent::SetResonance { amount } => engine.set_resonance(amount),
        ControlEvent::SetAttack { ms } => engine.set_attack_ms(ms),
        ControlEvent::SetDecay { ms } => engine.set_decay_ms(ms),
        ControlEvent::SetSustain { level } => engine.set_sustain(level),
        ControlEvent::SetRelease { ms } => engine.set_release_ms(ms),
        ControlEvent::SetWave { waveform } => engine.set_waveform(waveform),
    }
}

fn push_event(producer: &Arc<Mutex<Producer<ControlEvent>>>, event: ControlEvent) {
    // If the audio thread is behind, drop the event rather than blocking the audio path.
    // Contending briefly with the other control writer is fine; we never lock in the callback.
    if let Ok(mut guard) = producer.lock() {
        let _ = guard.push(event);
    }
}

fn try_open_first_midi_input(
    producer: Arc<Mutex<Producer<ControlEvent>>>,
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

    match midi_in.connect(
        port,
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

fn parse_midi_message(message: &[u8]) -> Option<ControlEvent> {
    if message.len() < 2 {
        return None;
    }

    let status = message[0];
    let note = message[1];
    let velocity = message.get(2).copied().unwrap_or(0);
    let kind = status & 0xF0;

    match kind {
        0x90 if velocity > 0 => Some(ControlEvent::NoteOn { note, velocity }),
        0x90 | 0x80 => Some(ControlEvent::NoteOff { note }),
        _ => None,
    }
}

fn print_keyboard_map() {
    println!("Keyboard map (C4 octave):");
    println!("  A W S E D F T G Y H U J K");
    println!("  C C# D D# E F F# G G# A A# B C");
}

fn print_param_help() {
    println!("Param commands:");
    println!("  cutoff <Hz>   res <0..1>   wave saw|square");
    println!("  attack <ms>   decay <ms>   sustain <0..1>   release <ms>");
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
    producer: &Arc<Mutex<Producer<ControlEvent>>>,
) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let result = keyboard_event_loop(producer);
    disable_raw_mode()?;
    // Move to a new line after raw mode so the shell prompt is clean.
    println!();
    result
}

fn keyboard_event_loop(
    producer: &Arc<Mutex<Producer<ControlEvent>>>,
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
                    match parse_param_command(line.trim()) {
                        Ok(Some(event)) => {
                            push_event(producer, event);
                            println!("ok");
                        }
                        Ok(None) => println!("(empty command)"),
                        Err(err) => println!("error: {err}"),
                    }
                    enable_raw_mode()?;
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if let Some(note) = key_to_note(code) {
                        if pressed.insert(note) {
                            push_event(
                                producer,
                                ControlEvent::NoteOn {
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
                            push_event(producer, ControlEvent::NoteOff { note });
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
    producer: &Arc<Mutex<Producer<ControlEvent>>>,
) -> Result<(), Box<dyn Error>> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("q") {
            break;
        }
        match parse_param_command(trimmed) {
            Ok(Some(event)) => {
                push_event(producer, event);
                println!("ok");
            }
            Ok(None) => {}
            Err(err) => println!("error: {err}"),
        }
    }
    Ok(())
}

fn parse_param_command(line: &str) -> Result<Option<ControlEvent>, String> {
    if line.is_empty() {
        return Ok(None);
    }

    let mut parts = line.split_whitespace();
    let cmd = parts
        .next()
        .ok_or_else(|| "expected a command".to_string())?
        .to_ascii_lowercase();
    let arg = parts.next();

    if parts.next().is_some() {
        return Err("too many arguments".to_string());
    }

    match cmd.as_str() {
        "cutoff" => {
            let hz = parse_f32_arg(arg, "cutoff")?;
            Ok(Some(ControlEvent::SetCutoff { hz }))
        }
        "res" | "resonance" => {
            let amount = parse_f32_arg(arg, "res")?;
            Ok(Some(ControlEvent::SetResonance { amount }))
        }
        "attack" => {
            let ms = parse_f32_arg(arg, "attack")?;
            Ok(Some(ControlEvent::SetAttack { ms }))
        }
        "decay" => {
            let ms = parse_f32_arg(arg, "decay")?;
            Ok(Some(ControlEvent::SetDecay { ms }))
        }
        "sustain" => {
            let level = parse_f32_arg(arg, "sustain")?;
            Ok(Some(ControlEvent::SetSustain { level }))
        }
        "release" => {
            let ms = parse_f32_arg(arg, "release")?;
            Ok(Some(ControlEvent::SetRelease { ms }))
        }
        "wave" => {
            let name = arg.ok_or_else(|| "wave needs saw or square".to_string())?;
            let waveform = match name.to_ascii_lowercase().as_str() {
                "saw" => Waveform::Saw,
                "square" | "sq" => Waveform::Square,
                other => return Err(format!("unknown wave '{other}' (use saw or square)")),
            };
            Ok(Some(ControlEvent::SetWave { waveform }))
        }
        other => Err(format!(
            "unknown command '{other}' (cutoff, res, attack, decay, sustain, release, wave)"
        )),
    }
}

fn parse_f32_arg(arg: Option<&str>, name: &str) -> Result<f32, String> {
    let raw = arg.ok_or_else(|| format!("{name} needs a number"))?;
    raw.parse::<f32>()
        .map_err(|_| format!("could not parse '{raw}' as a number for {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cutoff_and_wave() {
        match parse_param_command("cutoff 800").unwrap() {
            Some(ControlEvent::SetCutoff { hz }) => assert!((hz - 800.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("wave square").unwrap() {
            Some(ControlEvent::SetWave {
                waveform: Waveform::Square,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(parse_param_command("foo 1").is_err());
    }
}
