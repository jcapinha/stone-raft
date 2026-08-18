//! Shared laptop-host plumbing for `host-wsl` and `host-windows`.

use std::collections::HashSet;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SizedSample};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use engine::{Engine, Env3Dest, EnvelopeId, Waveform};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use rand::Rng;
use rtrb::{Producer, RingBuffer};

const EVENT_QUEUE_CAPACITY: usize = 128;
const KEYBOARD_VELOCITY: u8 = 100;
/// MIDI note for C4; letter-key map builds one octave up from here.
const KEYBOARD_ROOT_NOTE: u8 = 60;

const RANDOM_CUTOFF_MIN_HZ: f32 = 80.0;
const RANDOM_CUTOFF_MAX_HZ: f32 = 12_000.0;
const RANDOM_RES_MAX: f32 = 0.9;
const RANDOM_TIME_MIN_MS: f32 = 1.0;
const RANDOM_TIME_MAX_MS: f32 = 2_000.0;
const RANDOM_AMT_MIN: f32 = -4.0;
const RANDOM_AMT_MAX: f32 = 4.0;
const RANDOM_RES_AMT_MIN: f32 = -1.0;
const RANDOM_RES_AMT_MAX: f32 = 1.0;

const RANDOM_WAVEFORMS: [Waveform; 2] = [Waveform::Saw, Waveform::Square];
const RANDOM_ENV3_DESTS: [Env3Dest; 4] = [
    Env3Dest::Off,
    Env3Dest::Resonance,
    Env3Dest::Pitch,
    Env3Dest::Cutoff,
];

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
    SetFiltEnvAmt { amount: f32 },
    SetFiltEnvAttack { ms: f32 },
    SetFiltEnvDecay { ms: f32 },
    SetFiltEnvSustain { level: f32 },
    SetFiltEnvRelease { ms: f32 },
    SetEnv3Dest { dest: Env3Dest },
    SetEnv3Amt { amount: f32 },
    SetEnv3Attack { ms: f32 },
    SetEnv3Decay { ms: f32 },
    SetEnv3Sustain { level: f32 },
    SetEnv3Release { ms: f32 },
    EnvCopy,
    SetEnvLink { on: bool },
    SetEnvVel { amount: f32 },
}

struct ParamCommand {
    events: Vec<ControlEvent>,
    report: Option<String>,
}

#[derive(Clone, Copy)]
struct RandomAdsr {
    attack_ms: f32,
    decay_ms: f32,
    sustain: f32,
    release_ms: f32,
}

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
    if let Some(_midi_connection) = try_open_midi_input(midi_client_name, Arc::clone(&producer))? {
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
        ControlEvent::SetAttack { ms } => {
            engine.patch_envelope(EnvelopeId::Amp, |times| times.attack_ms = ms)
        }
        ControlEvent::SetDecay { ms } => {
            engine.patch_envelope(EnvelopeId::Amp, |times| times.decay_ms = ms)
        }
        ControlEvent::SetSustain { level } => {
            engine.patch_envelope(EnvelopeId::Amp, |times| times.sustain = level)
        }
        ControlEvent::SetRelease { ms } => {
            engine.patch_envelope(EnvelopeId::Amp, |times| times.release_ms = ms)
        }
        ControlEvent::SetWave { waveform } => engine.set_waveform(waveform),
        ControlEvent::SetFiltEnvAmt { amount } => engine.set_filtenv_amt(amount),
        ControlEvent::SetFiltEnvAttack { ms } => {
            engine.patch_envelope(EnvelopeId::Filter, |times| times.attack_ms = ms)
        }
        ControlEvent::SetFiltEnvDecay { ms } => {
            engine.patch_envelope(EnvelopeId::Filter, |times| times.decay_ms = ms)
        }
        ControlEvent::SetFiltEnvSustain { level } => {
            engine.patch_envelope(EnvelopeId::Filter, |times| times.sustain = level)
        }
        ControlEvent::SetFiltEnvRelease { ms } => {
            engine.patch_envelope(EnvelopeId::Filter, |times| times.release_ms = ms)
        }
        ControlEvent::SetEnv3Dest { dest } => engine.set_env3_dest(dest),
        ControlEvent::SetEnv3Amt { amount } => engine.set_env3_amt(amount),
        ControlEvent::SetEnv3Attack { ms } => {
            engine.patch_envelope(EnvelopeId::Assignable, |times| times.attack_ms = ms)
        }
        ControlEvent::SetEnv3Decay { ms } => {
            engine.patch_envelope(EnvelopeId::Assignable, |times| times.decay_ms = ms)
        }
        ControlEvent::SetEnv3Sustain { level } => {
            engine.patch_envelope(EnvelopeId::Assignable, |times| times.sustain = level)
        }
        ControlEvent::SetEnv3Release { ms } => {
            engine.patch_envelope(EnvelopeId::Assignable, |times| times.release_ms = ms)
        }
        ControlEvent::EnvCopy => engine.env_copy(),
        ControlEvent::SetEnvLink { on } => engine.set_env_link(on),
        ControlEvent::SetEnvVel { amount } => engine.set_envvel(amount),
    }
}

fn push_event(producer: &Arc<Mutex<Producer<ControlEvent>>>, event: ControlEvent) {
    // If the audio thread is behind, drop the event rather than blocking the audio path.
    // Contending briefly with the other control writer is fine; we never lock in the callback.
    if let Ok(mut guard) = producer.lock() {
        let _ = guard.push(event);
    }
}

fn try_open_midi_input(
    client_name: &str,
    producer: Arc<Mutex<Producer<ControlEvent>>>,
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
    println!(
        "  filtenvamt <signed octaves>   filtenvattack/decay/release <ms>   filtenvsustain <0..1>"
    );
    println!("  env3dest off|res|pitch|cutoff   env3amt <signed>");
    println!("  env3attack/decay/release <ms>   env3sustain <0..1>");
    println!("  envcopy   envlink on|off   envvel <0..1>");
    println!("  random    (fill all params; prints the patch)");
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

fn run_keyboard_loop(producer: &Arc<Mutex<Producer<ControlEvent>>>) -> Result<(), Box<dyn Error>> {
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
                    dispatch_param_line(producer, line.trim(), Some("(empty command)"));
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
        dispatch_param_line(producer, trimmed, None);
    }
    Ok(())
}

fn dispatch_param_line(
    producer: &Arc<Mutex<Producer<ControlEvent>>>,
    line: &str,
    empty_message: Option<&str>,
) {
    match parse_line_commands(line) {
        Ok(Some(command)) => {
            for event in command.events {
                push_event(producer, event);
            }
            println!("ok");
            if let Some(report) = command.report {
                print!("{report}");
            }
        }
        Ok(None) => {
            if let Some(message) = empty_message {
                println!("{message}");
            }
        }
        Err(err) => println!("error: {err}"),
    }
}

fn parse_line_commands(line: &str) -> Result<Option<ParamCommand>, String> {
    if line.is_empty() {
        return Ok(None);
    }

    let mut parts = line.split_whitespace();
    let cmd = parts
        .next()
        .ok_or_else(|| "expected a command".to_string())?
        .to_ascii_lowercase();
    if cmd == "random" {
        if parts.next().is_some() {
            return Err("too many arguments".to_string());
        }
        return Ok(Some(generate_random_patch(&mut rand::thread_rng())));
    }

    Ok(parse_param_command(line)?.map(|event| ParamCommand {
        events: vec![event],
        report: None,
    }))
}

fn log_uniform<R: Rng>(rng: &mut R, min: f32, max: f32) -> f32 {
    let log_min = min.ln();
    let log_max = max.ln();
    rng.gen_range(log_min..=log_max).exp().clamp(min, max)
}

fn random_adsr<R: Rng>(rng: &mut R) -> RandomAdsr {
    RandomAdsr {
        attack_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        decay_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        sustain: rng.gen_range(0.0..=1.0),
        release_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
    }
}

fn wave_name(waveform: Waveform) -> &'static str {
    match waveform {
        Waveform::Saw => "saw",
        Waveform::Square => "square",
    }
}

fn env3_dest_name(dest: Env3Dest) -> &'static str {
    match dest {
        Env3Dest::Off => "off",
        Env3Dest::Resonance => "res",
        Env3Dest::Pitch => "pitch",
        Env3Dest::Cutoff => "cutoff",
    }
}

fn generate_random_patch<R: Rng>(rng: &mut R) -> ParamCommand {
    let waveform = RANDOM_WAVEFORMS[rng.gen_range(0..RANDOM_WAVEFORMS.len())];
    let cutoff_hz = log_uniform(rng, RANDOM_CUTOFF_MIN_HZ, RANDOM_CUTOFF_MAX_HZ);
    let resonance = rng.gen_range(0.0..=RANDOM_RES_MAX);
    let amp = random_adsr(rng);
    let env_link = rng.gen_bool(0.5);
    let filter_env = if env_link { amp } else { random_adsr(rng) };
    let assign_env = if env_link { amp } else { random_adsr(rng) };
    let filtenv_amt = rng.gen_range(RANDOM_AMT_MIN..=RANDOM_AMT_MAX);
    let env3_dest = RANDOM_ENV3_DESTS[rng.gen_range(0..RANDOM_ENV3_DESTS.len())];
    let env3_amt = match env3_dest {
        Env3Dest::Resonance => rng.gen_range(RANDOM_RES_AMT_MIN..=RANDOM_RES_AMT_MAX),
        Env3Dest::Off | Env3Dest::Pitch | Env3Dest::Cutoff => {
            rng.gen_range(RANDOM_AMT_MIN..=RANDOM_AMT_MAX)
        }
    };
    let envvel = rng.gen_range(0.0..=1.0);
    let link_name = if env_link { "on" } else { "off" };

    let report = format!(
        "wave {}\n\
         cutoff {cutoff_hz:.0}\n\
         res {resonance:.2}\n\
         attack {:.0}\n\
         decay {:.0}\n\
         sustain {:.2}\n\
         release {:.0}\n\
         filtenvamt {filtenv_amt:.2}\n\
         filtenvattack {:.0}\n\
         filtenvdecay {:.0}\n\
         filtenvsustain {:.2}\n\
         filtenvrelease {:.0}\n\
         env3dest {}\n\
         env3amt {env3_amt:.2}\n\
         env3attack {:.0}\n\
         env3decay {:.0}\n\
         env3sustain {:.2}\n\
         env3release {:.0}\n\
         envvel {envvel:.2}\n\
         envlink {link_name}\n",
        wave_name(waveform),
        amp.attack_ms,
        amp.decay_ms,
        amp.sustain,
        amp.release_ms,
        filter_env.attack_ms,
        filter_env.decay_ms,
        filter_env.sustain,
        filter_env.release_ms,
        env3_dest_name(env3_dest),
        assign_env.attack_ms,
        assign_env.decay_ms,
        assign_env.sustain,
        assign_env.release_ms,
    );

    let mut events = vec![
        ControlEvent::SetWave { waveform },
        ControlEvent::SetCutoff { hz: cutoff_hz },
        ControlEvent::SetResonance { amount: resonance },
        ControlEvent::SetAttack { ms: amp.attack_ms },
        ControlEvent::SetDecay { ms: amp.decay_ms },
        ControlEvent::SetSustain { level: amp.sustain },
        ControlEvent::SetRelease { ms: amp.release_ms },
        ControlEvent::SetFiltEnvAmt {
            amount: filtenv_amt,
        },
        ControlEvent::SetEnv3Dest { dest: env3_dest },
        ControlEvent::SetEnv3Amt { amount: env3_amt },
        ControlEvent::SetEnvVel { amount: envvel },
    ];

    if env_link {
        events.push(ControlEvent::SetEnvLink { on: true });
    } else {
        events.push(ControlEvent::SetEnvLink { on: false });
        events.push(ControlEvent::SetFiltEnvAttack {
            ms: filter_env.attack_ms,
        });
        events.push(ControlEvent::SetFiltEnvDecay {
            ms: filter_env.decay_ms,
        });
        events.push(ControlEvent::SetFiltEnvSustain {
            level: filter_env.sustain,
        });
        events.push(ControlEvent::SetFiltEnvRelease {
            ms: filter_env.release_ms,
        });
        events.push(ControlEvent::SetEnv3Attack {
            ms: assign_env.attack_ms,
        });
        events.push(ControlEvent::SetEnv3Decay {
            ms: assign_env.decay_ms,
        });
        events.push(ControlEvent::SetEnv3Sustain {
            level: assign_env.sustain,
        });
        events.push(ControlEvent::SetEnv3Release {
            ms: assign_env.release_ms,
        });
    }

    ParamCommand {
        events,
        report: Some(report),
    }
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
        "filtenvamt" => {
            let amount = parse_f32_arg(arg, "filtenvamt")?;
            Ok(Some(ControlEvent::SetFiltEnvAmt { amount }))
        }
        "filtenvattack" => {
            let ms = parse_f32_arg(arg, "filtenvattack")?;
            Ok(Some(ControlEvent::SetFiltEnvAttack { ms }))
        }
        "filtenvdecay" => {
            let ms = parse_f32_arg(arg, "filtenvdecay")?;
            Ok(Some(ControlEvent::SetFiltEnvDecay { ms }))
        }
        "filtenvsustain" => {
            let level = parse_f32_arg(arg, "filtenvsustain")?;
            Ok(Some(ControlEvent::SetFiltEnvSustain { level }))
        }
        "filtenvrelease" => {
            let ms = parse_f32_arg(arg, "filtenvrelease")?;
            Ok(Some(ControlEvent::SetFiltEnvRelease { ms }))
        }
        "env3dest" => {
            let name =
                arg.ok_or_else(|| "env3dest needs off, res, pitch, or cutoff".to_string())?;
            let dest = match name.to_ascii_lowercase().as_str() {
                "off" => Env3Dest::Off,
                "res" | "resonance" => Env3Dest::Resonance,
                "pitch" => Env3Dest::Pitch,
                "cutoff" => Env3Dest::Cutoff,
                other => {
                    return Err(format!(
                        "unknown dest '{other}' (use off, res, pitch, or cutoff)"
                    ));
                }
            };
            Ok(Some(ControlEvent::SetEnv3Dest { dest }))
        }
        "env3amt" => {
            let amount = parse_f32_arg(arg, "env3amt")?;
            Ok(Some(ControlEvent::SetEnv3Amt { amount }))
        }
        "env3attack" => {
            let ms = parse_f32_arg(arg, "env3attack")?;
            Ok(Some(ControlEvent::SetEnv3Attack { ms }))
        }
        "env3decay" => {
            let ms = parse_f32_arg(arg, "env3decay")?;
            Ok(Some(ControlEvent::SetEnv3Decay { ms }))
        }
        "env3sustain" => {
            let level = parse_f32_arg(arg, "env3sustain")?;
            Ok(Some(ControlEvent::SetEnv3Sustain { level }))
        }
        "env3release" => {
            let ms = parse_f32_arg(arg, "env3release")?;
            Ok(Some(ControlEvent::SetEnv3Release { ms }))
        }
        "envcopy" => {
            if arg.is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(Some(ControlEvent::EnvCopy))
        }
        "envlink" => {
            let name = arg.ok_or_else(|| "envlink needs on or off".to_string())?;
            let on = match name.to_ascii_lowercase().as_str() {
                "on" => true,
                "off" => false,
                other => return Err(format!("unknown envlink '{other}' (use on or off)")),
            };
            Ok(Some(ControlEvent::SetEnvLink { on }))
        }
        "envvel" => {
            let amount = parse_f32_arg(arg, "envvel")?;
            Ok(Some(ControlEvent::SetEnvVel { amount }))
        }
        other => Err(format!(
            "unknown command '{other}' (cutoff, res, attack, decay, sustain, release, wave, filtenv*, env3*, envcopy, envlink, envvel, random)"
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
    use rand::SeedableRng;

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

    #[test]
    fn parses_filtenvamt_signed() {
        match parse_param_command("filtenvamt -2.5").unwrap() {
            Some(ControlEvent::SetFiltEnvAmt { amount }) => {
                assert!((amount + 2.5).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_env3dest_tokens_and_alias() {
        match parse_param_command("env3dest off").unwrap() {
            Some(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Off,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest res").unwrap() {
            Some(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Resonance,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest resonance").unwrap() {
            Some(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Resonance,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest pitch").unwrap() {
            Some(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Pitch,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest cutoff").unwrap() {
            Some(ControlEvent::SetEnv3Dest {
                dest: Env3Dest::Cutoff,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_env3dest() {
        let err = parse_param_command("env3dest pwm").unwrap_err();
        assert!(err.contains("unknown dest"));
    }

    #[test]
    fn parses_envcopy_and_envlink() {
        match parse_param_command("envcopy").unwrap() {
            Some(ControlEvent::EnvCopy) => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_param_command("envcopy extra").is_err());
        match parse_param_command("envlink on").unwrap() {
            Some(ControlEvent::SetEnvLink { on: true }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("envlink off").unwrap() {
            Some(ControlEvent::SetEnvLink { on: false }) => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_param_command("envlink maybe").is_err());
    }

    #[test]
    fn rejects_random_with_extra_args() {
        assert!(parse_line_commands("random extra").is_err());
    }

    #[test]
    fn parses_random_command() {
        let parsed = parse_line_commands("random")
            .unwrap()
            .expect("random should produce events");
        assert!(parsed.events.len() > 1);
        assert!(parsed.report.is_some());
    }

    #[test]
    fn random_command_prints_a_replayable_patch() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let patch = generate_random_patch(&mut rng);
        let report = patch.report.expect("random should print the patch");
        assert!(report.contains("wave "));
        assert!(report.contains("cutoff "));
        assert!(report.contains("envlink "));
        assert!(report.ends_with('\n'));
        assert!(!patch.events.is_empty());
    }

    #[test]
    fn random_patches_stay_in_range_and_respect_envlink() {
        for seed in 0..32 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let patch = generate_random_patch(&mut rng);
            let mut saw_link_on = false;
            let mut saw_link_off = false;
            let mut extra_env_times = 0usize;
            let mut env3_dest = Env3Dest::Off;

            for event in &patch.events {
                match event {
                    ControlEvent::SetCutoff { hz } => {
                        assert!(*hz >= RANDOM_CUTOFF_MIN_HZ && *hz <= RANDOM_CUTOFF_MAX_HZ);
                    }
                    ControlEvent::SetResonance { amount } => {
                        assert!(*amount >= 0.0 && *amount <= RANDOM_RES_MAX);
                    }
                    ControlEvent::SetAttack { ms }
                    | ControlEvent::SetDecay { ms }
                    | ControlEvent::SetRelease { ms } => {
                        assert!(*ms >= RANDOM_TIME_MIN_MS && *ms <= RANDOM_TIME_MAX_MS);
                    }
                    ControlEvent::SetFiltEnvAttack { ms }
                    | ControlEvent::SetFiltEnvDecay { ms }
                    | ControlEvent::SetFiltEnvRelease { ms }
                    | ControlEvent::SetEnv3Attack { ms }
                    | ControlEvent::SetEnv3Decay { ms }
                    | ControlEvent::SetEnv3Release { ms } => {
                        extra_env_times += 1;
                        assert!(*ms >= RANDOM_TIME_MIN_MS && *ms <= RANDOM_TIME_MAX_MS);
                    }
                    ControlEvent::SetSustain { level }
                    | ControlEvent::SetEnvVel { amount: level } => {
                        assert!(*level >= 0.0 && *level <= 1.0);
                    }
                    ControlEvent::SetFiltEnvSustain { level }
                    | ControlEvent::SetEnv3Sustain { level } => {
                        extra_env_times += 1;
                        assert!(*level >= 0.0 && *level <= 1.0);
                    }
                    ControlEvent::SetFiltEnvAmt { amount } => {
                        assert!(*amount >= RANDOM_AMT_MIN && *amount <= RANDOM_AMT_MAX);
                    }
                    ControlEvent::SetEnv3Dest { dest } => env3_dest = *dest,
                    ControlEvent::SetEnv3Amt { amount } => match env3_dest {
                        Env3Dest::Resonance => {
                            assert!(*amount >= RANDOM_RES_AMT_MIN && *amount <= RANDOM_RES_AMT_MAX);
                        }
                        Env3Dest::Off | Env3Dest::Pitch | Env3Dest::Cutoff => {
                            assert!(*amount >= RANDOM_AMT_MIN && *amount <= RANDOM_AMT_MAX);
                        }
                    },
                    ControlEvent::SetEnvLink { on: true } => saw_link_on = true,
                    ControlEvent::SetEnvLink { on: false } => saw_link_off = true,
                    ControlEvent::SetWave { .. }
                    | ControlEvent::NoteOn { .. }
                    | ControlEvent::NoteOff { .. }
                    | ControlEvent::EnvCopy => {}
                }
            }

            assert!(
                saw_link_on ^ saw_link_off,
                "seed {seed}: expected exactly one envlink setting"
            );
            if saw_link_on {
                assert_eq!(
                    extra_env_times, 0,
                    "seed {seed}: linked patch must not send extra envelope times"
                );
            } else {
                assert_eq!(
                    extra_env_times, 8,
                    "seed {seed}: unlinked patch should set filter and env3 times"
                );
            }
        }
    }
}
