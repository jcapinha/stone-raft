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
use engine::{
    AdsrTimes, ControlEvent, ENGINE_COUNT, EngineParams, Env3Dest, EnvelopeField, EnvelopeId,
    Mixer, MixerEvent, SlotEvent, SubOctaves, Waveform,
};
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
const RANDOM_VOL_MIN: f32 = 0.2;
const RANDOM_VOL_MAX: f32 = 1.0;

const RANDOM_PULSE_MIN: f32 = 0.05;
const RANDOM_PULSE_MAX: f32 = 0.95;
const RANDOM_ENV3_DESTS: [Env3Dest; 4] = [
    Env3Dest::Off,
    Env3Dest::Resonance,
    Env3Dest::Pitch,
    Env3Dest::Cutoff,
];

struct SlotShadow {
    params: EngineParams,
    enabled: bool,
    listen_channel: u8,
    volume: f32,
}

struct Session {
    current_slot: usize,
    shadows: [SlotShadow; ENGINE_COUNT],
}

impl Session {
    fn new() -> Self {
        Self {
            current_slot: 0,
            shadows: [
                SlotShadow {
                    params: EngineParams::default(),
                    enabled: true,
                    listen_channel: 1,
                    volume: 1.0,
                },
                SlotShadow {
                    params: EngineParams::default(),
                    enabled: false,
                    listen_channel: 2,
                    volume: 1.0,
                },
                SlotShadow {
                    params: EngineParams::default(),
                    enabled: false,
                    listen_channel: 3,
                    volume: 1.0,
                },
                SlotShadow {
                    params: EngineParams::default(),
                    enabled: false,
                    listen_channel: 4,
                    volume: 1.0,
                },
            ],
        }
    }

    fn apply_event(&mut self, event: MixerEvent) {
        match event {
            MixerEvent::ToSlot { slot, event } => {
                let index = slot as usize;
                if index >= ENGINE_COUNT {
                    return;
                }
                let shadow = &mut self.shadows[index];
                match event {
                    SlotEvent::Engine(control) => {
                        let _ = shadow.params.apply(control);
                    }
                    SlotEvent::SetEnabled { on } => shadow.enabled = on,
                    SlotEvent::SetListenChannel { channel } => {
                        shadow.listen_channel = channel.clamp(1, 16);
                    }
                    SlotEvent::SetVolume { amount } => {
                        shadow.volume = amount.clamp(0.0, 1.0);
                    }
                }
            }
            MixerEvent::MidiNoteOn { .. } | MixerEvent::MidiNoteOff { .. } => {}
        }
    }

    fn current_enabled(&self) -> bool {
        self.shadows[self.current_slot].enabled
    }
}

#[derive(Debug)]
enum PrintAfter {
    None,
    Status,
    Show { slot: usize },
    Report(String),
}

struct ParsedCommand {
    switch_current: Option<usize>,
    events: Vec<MixerEvent>,
    print: PrintAfter,
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

    let (producer, consumer) = RingBuffer::<MixerEvent>::new(EVENT_QUEUE_CAPACITY);
    // Mutex is only for MIDI callback vs terminal (never held on the audio path).
    let producer = Arc::new(Mutex::new(producer));
    let stream_config = supported_config.config();
    let mut session = Session::new();

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
        run_line_command_loop(&producer, &mut session)?;
    } else {
        println!("Using laptop keyboard.");
        print_keyboard_map();
        print_param_help();
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

fn print_param_help() {
    println!("Engine commands (1-based; space required, e.g. eng 2):");
    println!("  eng              print current on/off, ch, vol");
    println!("  eng <1..4>       switch current engine");
    println!("  on / off         enable or disable (off silences immediately)");
    println!("  ch <1..16>       MIDI listen channel");
    println!("  vol <0..1>       instance volume");
    println!("  show             print qualified patch from host copy");
    println!("  random           fill params + vol (0.2-1.0); prints eng N lines");
    println!("Param commands (optional eng N prefix is one-shot):");
    println!("  cutoff <Hz>   res <0..1>");
    println!("  sawvol|sawv <0..1>   squarevol|sqvol <0..1>   trianglevol|trivol <0..1>   sinevol|sinvol <0..1>");
    println!("  wave saw|square|triangle|sine   pulse <0.05..0.95>");
    println!("  subvol <0..1>   suboct 1|2");
    println!("  attack <ms>   decay <ms>   sustain <0..1>   release <ms>");
    println!(
        "  filtenvamt <signed octaves>   filtenvattack/decay/release <ms>   filtenvsustain <0..1>"
    );
    println!("  env3dest off|res|pitch|cutoff   env3amt <signed>");
    println!("  env3attack/decay/release <ms>   env3sustain <0..1>");
    println!("  envcopy   envlink on|off   envvel <0..1>");
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
    session: &mut Session,
) -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let result = keyboard_event_loop(producer, session);
    disable_raw_mode()?;
    // Move to a new line after raw mode so the shell prompt is clean.
    println!();
    result
}

fn keyboard_note_event(session: &Session, note: u8, on: bool) -> MixerEvent {
    let control = if on {
        ControlEvent::NoteOn {
            note,
            velocity: KEYBOARD_VELOCITY,
        }
    } else {
        ControlEvent::NoteOff { note }
    };
    MixerEvent::ToSlot {
        slot: session.current_slot as u8,
        event: SlotEvent::Engine(control),
    }
}

fn keyboard_event_loop(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut Session,
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
                                push_event(producer, keyboard_note_event(session, note, true));
                            } else {
                                print!(
                                    "\r\nengine {} is off; type: on\r\n",
                                    session.current_slot + 1
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
                            push_event(producer, keyboard_note_event(session, note, false));
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
    session: &mut Session,
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

fn apply_parsed(session: &mut Session, command: &ParsedCommand) {
    if let Some(slot) = command.switch_current {
        session.current_slot = slot;
    }
    for event in &command.events {
        session.apply_event(*event);
    }
}

fn dispatch_param_line(
    producer: &Arc<Mutex<Producer<MixerEvent>>>,
    session: &mut Session,
    line: &str,
    empty_message: Option<&str>,
) {
    match parse_line_commands(line, session.current_slot) {
        Ok(Some(command)) => {
            apply_parsed(session, &command);
            for event in command.events {
                push_event(producer, event);
            }
            println!("ok");
            match command.print {
                PrintAfter::None => {}
                PrintAfter::Status => print!("{}", format_eng_status(session)),
                PrintAfter::Show { slot } => print!("{}", format_show(session, slot)),
                PrintAfter::Report(report) => print!("{report}"),
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

fn is_glued_eng_token(cmd: &str) -> bool {
    match cmd.strip_prefix("eng") {
        Some(rest) if !rest.is_empty() => rest.chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

fn parse_engine_number(raw: &str) -> Result<usize, String> {
    let n: usize = raw
        .parse()
        .map_err(|_| format!("eng needs a number 1 through {ENGINE_COUNT}"))?;
    if !(1..=ENGINE_COUNT).contains(&n) {
        return Err(format!("eng needs a number 1 through {ENGINE_COUNT}"));
    }
    Ok(n - 1)
}

fn parse_listen_channel(raw: &str) -> Result<u8, String> {
    let n: u8 = raw
        .parse()
        .map_err(|_| "ch needs a channel 1 through 16".to_string())?;
    if !(1..=16).contains(&n) {
        return Err("ch needs a channel 1 through 16".to_string());
    }
    Ok(n)
}

fn to_slot(slot: usize, event: SlotEvent) -> MixerEvent {
    MixerEvent::ToSlot {
        slot: slot as u8,
        event,
    }
}

fn parse_line_commands(line: &str, current_slot: usize) -> Result<Option<ParsedCommand>, String> {
    if line.is_empty() {
        return Ok(None);
    }

    let tokens: Vec<&str> = line.split_whitespace().collect();
    let first = tokens[0].to_ascii_lowercase();
    if is_glued_eng_token(&first) {
        return Err("use 'eng N' with a space (for example eng 2)".to_string());
    }

    if first == "eng" {
        if tokens.len() == 1 {
            return Ok(Some(ParsedCommand {
                switch_current: None,
                events: Vec::new(),
                print: PrintAfter::Status,
            }));
        }
        let slot = parse_engine_number(tokens[1])?;
        if tokens.len() == 2 {
            return Ok(Some(ParsedCommand {
                switch_current: Some(slot),
                events: Vec::new(),
                print: PrintAfter::Status,
            }));
        }
        let rest = tokens[2..].join(" ");
        return parse_targeted_command(&rest, slot).map(Some);
    }

    parse_targeted_command(line, current_slot).map(Some)
}

fn parse_targeted_command(line: &str, slot: usize) -> Result<ParsedCommand, String> {
    let mut parts = line.split_whitespace();
    let cmd = parts
        .next()
        .ok_or_else(|| "expected a command".to_string())?
        .to_ascii_lowercase();
    if is_glued_eng_token(&cmd) {
        return Err("use 'eng N' with a space (for example eng 2)".to_string());
    }

    match cmd.as_str() {
        "on" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_slot(slot, SlotEvent::SetEnabled { on: true })],
                print: PrintAfter::None,
            })
        }
        "off" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_slot(slot, SlotEvent::SetEnabled { on: false })],
                print: PrintAfter::None,
            })
        }
        "ch" => {
            let raw = parts
                .next()
                .ok_or_else(|| "ch needs a channel 1 through 16".to_string())?;
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            let channel = parse_listen_channel(raw)?;
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_slot(slot, SlotEvent::SetListenChannel { channel })],
                print: PrintAfter::None,
            })
        }
        "vol" => {
            let amount = parse_f32_arg(parts.next(), "vol")?;
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_slot(slot, SlotEvent::SetVolume { amount })],
                print: PrintAfter::None,
            })
        }
        "show" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(ParsedCommand {
                switch_current: None,
                events: Vec::new(),
                print: PrintAfter::Show { slot },
            })
        }
        "random" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(generate_random_patch(&mut rand::thread_rng(), slot))
        }
        _ => {
            let event =
                parse_param_command(line)?.ok_or_else(|| "expected a command".to_string())?;
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_slot(slot, SlotEvent::Engine(event))],
                print: PrintAfter::None,
            })
        }
    }
}

fn log_uniform<R: Rng>(rng: &mut R, min: f32, max: f32) -> f32 {
    let log_min = min.ln();
    let log_max = max.ln();
    rng.gen_range(log_min..=log_max).exp().clamp(min, max)
}

fn random_adsr<R: Rng>(rng: &mut R) -> AdsrTimes {
    AdsrTimes {
        attack_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        decay_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
        sustain: rng.gen_range(0.0..=1.0),
        release_ms: log_uniform(rng, RANDOM_TIME_MIN_MS, RANDOM_TIME_MAX_MS),
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

fn qualified(n: usize, rest: &str) -> String {
    format!("eng {n} {rest}\n")
}

fn qualify_block(n: usize, body: &str) -> String {
    body.lines()
        .filter(|line| !line.is_empty())
        .map(|line| qualified(n, line))
        .collect()
}

fn format_param_lines(params: &EngineParams) -> String {
    let link_name = if params.env_link { "on" } else { "off" };
    format!(
        "sawvol {:.2}\n\
         squarevol {:.2}\n\
         trianglevol {:.2}\n\
         sinevol {:.2}\n\
         pulse {:.2}\n\
         subvol {:.2}\n\
         suboct {}\n\
         cutoff {:.0}\n\
         res {:.2}\n\
         attack {:.0}\n\
         decay {:.0}\n\
         sustain {:.2}\n\
         release {:.0}\n\
         filtenvamt {:.2}\n\
         filtenvattack {:.0}\n\
         filtenvdecay {:.0}\n\
         filtenvsustain {:.2}\n\
         filtenvrelease {:.0}\n\
         env3dest {}\n\
         env3amt {:.2}\n\
         env3attack {:.0}\n\
         env3decay {:.0}\n\
         env3sustain {:.2}\n\
         env3release {:.0}\n\
         envvel {:.2}\n\
         envlink {link_name}\n",
        params.saw_vol,
        params.square_vol,
        params.triangle_vol,
        params.sine_vol,
        params.pulse_width,
        params.sub_vol,
        params.sub_octaves.as_u8(),
        params.cutoff_hz,
        params.resonance,
        params.amp.attack_ms,
        params.amp.decay_ms,
        params.amp.sustain,
        params.amp.release_ms,
        params.filtenv_amt,
        params.filter_env.attack_ms,
        params.filter_env.decay_ms,
        params.filter_env.sustain,
        params.filter_env.release_ms,
        env3_dest_name(params.env3_dest),
        params.env3_amt,
        params.assign_env.attack_ms,
        params.assign_env.decay_ms,
        params.assign_env.sustain,
        params.assign_env.release_ms,
        params.envvel,
    )
}

fn format_eng_status(session: &Session) -> String {
    let n = session.current_slot + 1;
    let shadow = &session.shadows[session.current_slot];
    let state = if shadow.enabled { "on" } else { "off" };
    format!(
        "{}{}{}",
        qualified(n, state),
        qualified(n, &format!("ch {}", shadow.listen_channel)),
        qualified(n, &format!("vol {:.2}", shadow.volume)),
    )
}

fn format_show(session: &Session, slot: usize) -> String {
    let n = slot + 1;
    let shadow = &session.shadows[slot];
    let state = if shadow.enabled { "on" } else { "off" };
    let mut out = String::new();
    out.push_str(&qualified(n, state));
    out.push_str(&qualified(n, &format!("ch {}", shadow.listen_channel)));
    out.push_str(&qualified(n, &format!("vol {:.2}", shadow.volume)));
    out.push_str(&qualify_block(n, &format_param_lines(&shadow.params)));
    out
}

fn wrap_engine(slot: usize, event: ControlEvent) -> MixerEvent {
    to_slot(slot, SlotEvent::Engine(event))
}

fn generate_random_patch<R: Rng>(rng: &mut R, slot: usize) -> ParsedCommand {
    let saw_vol = rng.gen_range(0.0..=1.0);
    let square_vol = rng.gen_range(0.0..=1.0);
    let triangle_vol = rng.gen_range(0.0..=1.0);
    let sine_vol = rng.gen_range(0.0..=1.0);
    let pulse_width = rng.gen_range(RANDOM_PULSE_MIN..=RANDOM_PULSE_MAX);
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
    let sub_vol = rng.gen_range(0.0..=1.0);
    let sub_octaves = if rng.gen_bool(0.5) {
        SubOctaves::One
    } else {
        SubOctaves::Two
    };
    let volume = rng.gen_range(RANDOM_VOL_MIN..=RANDOM_VOL_MAX);
    let params = EngineParams {
        saw_vol,
        square_vol,
        triangle_vol,
        sine_vol,
        pulse_width,
        cutoff_hz,
        resonance,
        amp,
        filter_env,
        assign_env,
        filtenv_amt,
        env3_amt,
        env3_dest,
        env_link,
        envvel,
        sub_vol,
        sub_octaves,
    };
    let n = slot + 1;
    let mut report = qualified(n, &format!("vol {volume:.2}"));
    report.push_str(&qualify_block(n, &format_param_lines(&params)));

    let mut events = vec![
        wrap_engine(slot, ControlEvent::SetSawVol { amount: saw_vol }),
        wrap_engine(slot, ControlEvent::SetSquareVol { amount: square_vol }),
        wrap_engine(
            slot,
            ControlEvent::SetTriangleVol {
                amount: triangle_vol,
            },
        ),
        wrap_engine(slot, ControlEvent::SetSineVol { amount: sine_vol }),
        wrap_engine(slot, ControlEvent::SetPulse { width: pulse_width }),
        wrap_engine(slot, ControlEvent::SetSubVol { amount: sub_vol }),
        wrap_engine(slot, ControlEvent::SetSubOct { octaves: sub_octaves }),
        wrap_engine(slot, ControlEvent::SetCutoff { hz: cutoff_hz }),
        wrap_engine(slot, ControlEvent::SetResonance { amount: resonance }),
        wrap_engine(
            slot,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Amp,
                times: amp,
            },
        ),
        wrap_engine(
            slot,
            ControlEvent::SetFiltEnvAmt {
                amount: filtenv_amt,
            },
        ),
        wrap_engine(slot, ControlEvent::SetEnv3Dest { dest: env3_dest }),
        wrap_engine(slot, ControlEvent::SetEnv3Amt { amount: env3_amt }),
        wrap_engine(slot, ControlEvent::SetEnvVel { amount: envvel }),
        to_slot(slot, SlotEvent::SetVolume { amount: volume }),
    ];

    if env_link {
        events.push(wrap_engine(slot, ControlEvent::SetEnvLink { on: true }));
    } else {
        events.push(wrap_engine(slot, ControlEvent::SetEnvLink { on: false }));
        events.push(wrap_engine(
            slot,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Filter,
                times: filter_env,
            },
        ));
        events.push(wrap_engine(
            slot,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Assignable,
                times: assign_env,
            },
        ));
    }

    ParsedCommand {
        switch_current: None,
        events,
        print: PrintAfter::Report(report),
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
        "attack" => envelope_patch(EnvelopeId::Amp, EnvelopeField::Attack, arg, "attack"),
        "decay" => envelope_patch(EnvelopeId::Amp, EnvelopeField::Decay, arg, "decay"),
        "sustain" => envelope_patch(EnvelopeId::Amp, EnvelopeField::Sustain, arg, "sustain"),
        "release" => envelope_patch(EnvelopeId::Amp, EnvelopeField::Release, arg, "release"),
        "sawvol" | "sawv" => {
            let amount = parse_f32_arg(arg, "sawvol")?;
            Ok(Some(ControlEvent::SetSawVol { amount }))
        }
        "squarevol" | "sqvol" => {
            let amount = parse_f32_arg(arg, "squarevol")?;
            Ok(Some(ControlEvent::SetSquareVol { amount }))
        }
        "trianglevol" | "trivol" => {
            let amount = parse_f32_arg(arg, "trianglevol")?;
            Ok(Some(ControlEvent::SetTriangleVol { amount }))
        }
        "sinevol" | "sinvol" => {
            let amount = parse_f32_arg(arg, "sinevol")?;
            Ok(Some(ControlEvent::SetSineVol { amount }))
        }
        "wave" => {
            let name = arg.ok_or_else(|| {
                "wave needs saw, square, triangle, or sine".to_string()
            })?;
            let waveform = match name.to_ascii_lowercase().as_str() {
                "saw" => Waveform::Saw,
                "square" | "sq" => Waveform::Square,
                "triangle" | "tri" => Waveform::Triangle,
                "sine" | "sin" => Waveform::Sine,
                other => {
                    return Err(format!(
                        "unknown wave '{other}' (use saw, square, triangle, or sine)"
                    ));
                }
            };
            Ok(Some(ControlEvent::SetWave { waveform }))
        }
        "pulse" => {
            let width = parse_f32_arg(arg, "pulse")?;
            Ok(Some(ControlEvent::SetPulse { width }))
        }
        "subvol" => {
            let amount = parse_f32_arg(arg, "subvol")?;
            Ok(Some(ControlEvent::SetSubVol { amount }))
        }
        "suboct" => {
            let raw = arg.ok_or_else(|| "suboct needs 1 or 2".to_string())?;
            let octaves = match raw {
                "1" => SubOctaves::One,
                "2" => SubOctaves::Two,
                other => {
                    return Err(format!("unknown suboct '{other}' (use 1 or 2)"));
                }
            };
            Ok(Some(ControlEvent::SetSubOct { octaves }))
        }
        "filtenvamt" => {
            let amount = parse_f32_arg(arg, "filtenvamt")?;
            Ok(Some(ControlEvent::SetFiltEnvAmt { amount }))
        }
        "filtenvattack" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Attack,
            arg,
            "filtenvattack",
        ),
        "filtenvdecay" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Decay,
            arg,
            "filtenvdecay",
        ),
        "filtenvsustain" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Sustain,
            arg,
            "filtenvsustain",
        ),
        "filtenvrelease" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Release,
            arg,
            "filtenvrelease",
        ),
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
        "env3attack" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Attack,
            arg,
            "env3attack",
        ),
        "env3decay" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Decay,
            arg,
            "env3decay",
        ),
        "env3sustain" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Sustain,
            arg,
            "env3sustain",
        ),
        "env3release" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Release,
            arg,
            "env3release",
        ),
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
            "unknown command '{other}' (eng, on, off, ch, vol, show, cutoff, res, attack, decay, sustain, release, sawvol, squarevol, trianglevol, sinevol, wave, pulse, subvol, suboct, filtenv*, env3*, envcopy, envlink, envvel, random)"
        )),
    }
}

fn envelope_patch(
    which: EnvelopeId,
    field: EnvelopeField,
    arg: Option<&str>,
    name: &str,
) -> Result<Option<ControlEvent>, String> {
    let value = parse_f32_arg(arg, name)?;
    Ok(Some(ControlEvent::PatchEnvelope {
        which,
        field,
        value,
    }))
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
        match parse_param_command("wave tri").unwrap() {
            Some(ControlEvent::SetWave {
                waveform: Waveform::Triangle,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("wave sin").unwrap() {
            Some(ControlEvent::SetWave {
                waveform: Waveform::Sine,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sawvol 0.5").unwrap() {
            Some(ControlEvent::SetSawVol { amount }) => {
                assert!((amount - 0.5).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sawv 0.3").unwrap() {
            Some(ControlEvent::SetSawVol { amount }) => {
                assert!((amount - 0.3).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sqvol 0.7").unwrap() {
            Some(ControlEvent::SetSquareVol { amount }) => {
                assert!((amount - 0.7).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("trivol 0.2").unwrap() {
            Some(ControlEvent::SetTriangleVol { amount }) => {
                assert!((amount - 0.2).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sinvol 0.8").unwrap() {
            Some(ControlEvent::SetSineVol { amount }) => {
                assert!((amount - 0.8).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("pulse 0.25").unwrap() {
            Some(ControlEvent::SetPulse { width }) => {
                assert!((width - 0.25).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("subvol 0.4").unwrap() {
            Some(ControlEvent::SetSubVol { amount }) => {
                assert!((amount - 0.4).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("suboct 2").unwrap() {
            Some(ControlEvent::SetSubOct {
                octaves: SubOctaves::Two,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("suboct 1").unwrap() {
            Some(ControlEvent::SetSubOct {
                octaves: SubOctaves::One,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_param_command("suboct 3").is_err());
        assert!(parse_param_command("suboct 0").is_err());
    }

    #[test]
    fn parses_envelope_time_commands_as_patches() {
        match parse_param_command("attack 12").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Amp,
                field: EnvelopeField::Attack,
                value,
            }) => assert!((value - 12.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("filtenvattack 50").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Filter,
                field: EnvelopeField::Attack,
                value,
            }) => assert!((value - 50.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3decay 80").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Assignable,
                field: EnvelopeField::Decay,
                value,
            }) => assert!((value - 80.0).abs() < f32::EPSILON),
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

    fn report_text(command: &ParsedCommand) -> &str {
        match &command.print {
            PrintAfter::Report(s) => s,
            other => panic!("expected report, got {other:?}"),
        }
    }

    #[test]
    fn rejects_random_with_extra_args() {
        assert!(parse_line_commands("random extra", 0).is_err());
    }

    #[test]
    fn parses_random_command() {
        let parsed = parse_line_commands("random", 0)
            .unwrap()
            .expect("random should produce events");
        assert!(parsed.events.len() > 1);
        assert!(matches!(parsed.print, PrintAfter::Report(_)));
    }

    #[test]
    fn random_command_prints_a_replayable_patch() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let patch = generate_random_patch(&mut rng, 0);
        let report = report_text(&patch);
        assert!(report.contains("eng "));
        assert!(report.contains("vol"));
        assert!(report.contains("sawvol "));
        assert!(report.contains("squarevol "));
        assert!(report.contains("trianglevol "));
        assert!(report.contains("sinevol "));
        assert!(report.contains("pulse "));
        assert!(report.contains("subvol "));
        assert!(report.contains("suboct "));
        assert!(report.contains("cutoff "));
        assert!(report.contains("envlink "));
        assert!(report.ends_with('\n'));
        assert!(!patch.events.is_empty());
    }

    #[test]
    fn random_patches_stay_in_range_and_respect_envlink() {
        for seed in 0..32 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let patch = generate_random_patch(&mut rng, 1);
            let mut saw_link_on = false;
            let mut saw_link_off = false;
            let mut extra_env_times = 0usize;
            let mut env3_dest = Env3Dest::Off;
            let mut saw_volume = false;

            let mut saw_osc_levels = 0usize;
            for event in &patch.events {
                match event {
                    MixerEvent::ToSlot { slot, event } => {
                        assert_eq!(*slot, 1);
                        match event {
                            SlotEvent::SetEnabled { .. } | SlotEvent::SetListenChannel { .. } => {
                                panic!(
                                    "seed {seed}: random must not change enabled or listen channel"
                                );
                            }
                            SlotEvent::SetVolume { amount } => {
                                assert!(
                                    *amount >= RANDOM_VOL_MIN && *amount <= RANDOM_VOL_MAX,
                                    "seed {seed}: volume {amount} out of range"
                                );
                                saw_volume = true;
                            }
                            SlotEvent::Engine(control) => match control {
                                ControlEvent::SetCutoff { hz } => {
                                    assert!(
                                        *hz >= RANDOM_CUTOFF_MIN_HZ && *hz <= RANDOM_CUTOFF_MAX_HZ
                                    );
                                }
                                ControlEvent::SetResonance { amount } => {
                                    assert!(*amount >= 0.0 && *amount <= RANDOM_RES_MAX);
                                }
                                ControlEvent::SetEnvelope { which, times } => {
                                    assert!(
                                        times.attack_ms >= RANDOM_TIME_MIN_MS
                                            && times.attack_ms <= RANDOM_TIME_MAX_MS
                                    );
                                    assert!(
                                        times.decay_ms >= RANDOM_TIME_MIN_MS
                                            && times.decay_ms <= RANDOM_TIME_MAX_MS
                                    );
                                    assert!(
                                        times.release_ms >= RANDOM_TIME_MIN_MS
                                            && times.release_ms <= RANDOM_TIME_MAX_MS
                                    );
                                    assert!(times.sustain >= 0.0 && times.sustain <= 1.0);
                                    match which {
                                        EnvelopeId::Amp => {}
                                        EnvelopeId::Filter | EnvelopeId::Assignable => {
                                            extra_env_times += 1
                                        }
                                    }
                                }
                                ControlEvent::SetEnvVel { amount } => {
                                    assert!(*amount >= 0.0 && *amount <= 1.0);
                                }
                                ControlEvent::SetFiltEnvAmt { amount } => {
                                    assert!(*amount >= RANDOM_AMT_MIN && *amount <= RANDOM_AMT_MAX);
                                }
                                ControlEvent::SetEnv3Dest { dest } => env3_dest = *dest,
                                ControlEvent::SetEnv3Amt { amount } => match env3_dest {
                                    Env3Dest::Resonance => {
                                        assert!(
                                            *amount >= RANDOM_RES_AMT_MIN
                                                && *amount <= RANDOM_RES_AMT_MAX
                                        );
                                    }
                                    Env3Dest::Off | Env3Dest::Pitch | Env3Dest::Cutoff => {
                                        assert!(
                                            *amount >= RANDOM_AMT_MIN && *amount <= RANDOM_AMT_MAX
                                        );
                                    }
                                },
                                ControlEvent::SetEnvLink { on: true } => saw_link_on = true,
                                ControlEvent::SetEnvLink { on: false } => saw_link_off = true,
                                ControlEvent::SetPulse { width } => {
                                    assert!(
                                        *width >= RANDOM_PULSE_MIN && *width <= RANDOM_PULSE_MAX,
                                        "seed {seed}: pulse {width} out of range"
                                    );
                                }
                                ControlEvent::SetSubVol { amount } => {
                                    assert!(
                                        *amount >= 0.0 && *amount <= 1.0,
                                        "seed {seed}: subvol {amount} out of range"
                                    );
                                }
                                ControlEvent::SetSubOct { octaves } => {
                                    assert!(
                                        matches!(octaves, SubOctaves::One | SubOctaves::Two),
                                        "seed {seed}: unexpected suboct {octaves:?}"
                                    );
                                }
                                ControlEvent::SetSawVol { amount }
                                | ControlEvent::SetSquareVol { amount }
                                | ControlEvent::SetTriangleVol { amount }
                                | ControlEvent::SetSineVol { amount } => {
                                    assert!(
                                        *amount >= 0.0 && *amount <= 1.0,
                                        "seed {seed}: osc level {amount} out of range"
                                    );
                                    saw_osc_levels += 1;
                                }
                                ControlEvent::SetWave { .. }
                                | ControlEvent::NoteOn { .. }
                                | ControlEvent::NoteOff { .. }
                                | ControlEvent::EnvCopy
                                | ControlEvent::PatchEnvelope { .. } => {}
                            },
                        }
                    }
                    MixerEvent::MidiNoteOn { .. } | MixerEvent::MidiNoteOff { .. } => {
                        panic!("seed {seed}: random must not emit MIDI events");
                    }
                }
            }

            assert!(saw_volume, "seed {seed}: random must include volume");
            assert_eq!(
                saw_osc_levels, 4,
                "seed {seed}: random must emit four at-pitch osc level events"
            );
            let report = report_text(&patch);
            assert!(report.contains("eng "));
            assert!(report.contains("vol"));
            assert!(report.contains("sawvol "));
            assert!(report.contains("squarevol "));
            assert!(report.contains("subvol "));
            assert!(report.contains("suboct "));
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
                    extra_env_times, 2,
                    "seed {seed}: unlinked patch should set filter and assignable times"
                );
            }
        }
    }

    #[test]
    fn eng_2_cutoff_is_oneshot() {
        let mut session = Session::new();
        assert_eq!(session.current_slot, 0);
        let parsed = parse_line_commands("eng 2 cutoff 800", session.current_slot)
            .unwrap()
            .expect("command");
        assert!(parsed.switch_current.is_none());
        match parsed.events.as_slice() {
            [
                MixerEvent::ToSlot {
                    slot: 1,
                    event: SlotEvent::Engine(ControlEvent::SetCutoff { hz }),
                },
            ] => assert!((*hz - 800.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        apply_parsed(&mut session, &parsed);
        assert_eq!(session.current_slot, 0);
        assert!((session.shadows[1].params.cutoff_hz - 800.0).abs() < f32::EPSILON);
    }

    #[test]
    fn glued_eng2_errors() {
        assert!(parse_line_commands("eng2", 0).is_err());
        assert!(parse_line_commands("eng2 cutoff 800", 0).is_err());
    }

    #[test]
    fn eng_out_of_range_errors() {
        assert!(parse_line_commands("eng 0", 0).is_err());
        assert!(parse_line_commands("eng 5", 0).is_err());
    }

    #[test]
    fn ch_out_of_range_errors() {
        assert!(parse_line_commands("ch 0", 0).is_err());
        assert!(parse_line_commands("ch 17", 0).is_err());
    }

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

    #[test]
    fn eng_2_random_does_not_switch_current_or_routing() {
        let mut session = Session::new();
        let parsed = parse_line_commands("eng 2 random", 0)
            .unwrap()
            .expect("random");
        assert!(parsed.switch_current.is_none());
        apply_parsed(&mut session, &parsed);
        assert_eq!(session.current_slot, 0);
        assert!(!session.shadows[1].enabled);
        assert_eq!(session.shadows[1].listen_channel, 2);
        assert!(session.shadows[1].volume >= RANDOM_VOL_MIN);
        assert!(session.shadows[1].volume <= RANDOM_VOL_MAX);
        let report = report_text(&parsed);
        assert!(report.contains("eng 2 "));
        assert!(report.contains("vol"));
        assert!(!report.contains("eng 2 on"));
        assert!(!report.contains("eng 2 off"));
        assert!(!report.contains("eng 2 ch "));
    }

    #[test]
    fn show_includes_subvol_and_suboct() {
        let mut session = Session::new();
        let parsed = parse_line_commands("subvol 0.35", 0)
            .unwrap()
            .expect("subvol");
        apply_parsed(&mut session, &parsed);
        let parsed = parse_line_commands("suboct 2", 0)
            .unwrap()
            .expect("suboct");
        apply_parsed(&mut session, &parsed);
        let shown = format_show(&session, 0);
        assert!(shown.contains("subvol 0.35"));
        assert!(shown.contains("suboct 2"));
        assert!((session.shadows[0].params.sub_vol - 0.35).abs() < f32::EPSILON);
        assert_eq!(session.shadows[0].params.sub_octaves, SubOctaves::Two);
    }
}
