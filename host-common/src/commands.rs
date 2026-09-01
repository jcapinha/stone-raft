//! Terminal command parsing and host-side engine state.

use engine::{
    AdsrTimes, AssignableDest, ControlEvent, ENGINE_COUNT, EngineParams, EnvelopeField, EnvelopeId,
    InstanceEvent, MixerEvent, SubOctaves, Waveform,
};
use rand::Rng;

const KEYBOARD_VELOCITY: u8 = 100;

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
const RANDOM_ASSIGNABLE_DESTS: [AssignableDest; 4] = [
    AssignableDest::Off,
    AssignableDest::Resonance,
    AssignableDest::Pitch,
    AssignableDest::Cutoff,
];

struct InstanceShadow {
    params: EngineParams,
    enabled: bool,
    listen_channel: u8,
    volume: f32,
}

pub(crate) struct CommandSession {
    current_instance: usize,
    shadows: [InstanceShadow; ENGINE_COUNT],
}

impl CommandSession {
    pub(crate) fn new() -> Self {
        Self {
            current_instance: 1,
            shadows: [
                InstanceShadow {
                    params: EngineParams::default(),
                    enabled: true,
                    listen_channel: 1,
                    volume: 1.0,
                },
                InstanceShadow {
                    params: EngineParams::default(),
                    enabled: false,
                    listen_channel: 2,
                    volume: 1.0,
                },
                InstanceShadow {
                    params: EngineParams::default(),
                    enabled: false,
                    listen_channel: 3,
                    volume: 1.0,
                },
                InstanceShadow {
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
            MixerEvent::ToInstance { instance, event } => {
                let index = (instance as usize).wrapping_sub(1);
                if index >= ENGINE_COUNT {
                    return;
                }
                let shadow = &mut self.shadows[index];
                match event {
                    InstanceEvent::Engine(control) => {
                        let _ = shadow.params.apply(control);
                    }
                    InstanceEvent::SetEnabled { on } => shadow.enabled = on,
                    InstanceEvent::SetListenChannel { channel } => {
                        shadow.listen_channel = channel.clamp(1, 16);
                    }
                    InstanceEvent::SetVolume { amount } => {
                        shadow.volume = amount.clamp(0.0, 1.0);
                    }
                }
            }
            MixerEvent::MidiNoteOn { .. } | MixerEvent::MidiNoteOff { .. } => {}
        }
    }

    pub(crate) fn current_enabled(&self) -> bool {
        self.shadows[self.current_instance - 1].enabled
    }
}

#[derive(Debug)]
enum PrintAfter {
    None,
    Status,
    Show { instance: usize },
    Report(String),
}

struct ParsedCommand {
    switch_current: Option<usize>,
    events: Vec<MixerEvent>,
    print: PrintAfter,
}

#[derive(Debug)]
pub(crate) enum CommandOutcome {
    Empty,
    Applied {
        events: Vec<MixerEvent>,
        text: String,
    },
    Error(String),
}

impl CommandSession {
    pub(crate) fn handle(&mut self, line: &str) -> CommandOutcome {
        match parse_line_commands(line, self.current_instance) {
            Ok(Some(command)) => {
                apply_parsed(self, &command);
                let text = match command.print {
                    PrintAfter::None => String::new(),
                    PrintAfter::Status => format_eng_status(self),
                    PrintAfter::Show { instance } => format_show(self, instance),
                    PrintAfter::Report(report) => report,
                };
                CommandOutcome::Applied {
                    events: command.events,
                    text,
                }
            }
            Ok(None) => CommandOutcome::Empty,
            Err(err) => CommandOutcome::Error(err),
        }
    }

    pub(crate) fn current_engine_number(&self) -> usize {
        self.current_instance
    }

    pub(crate) fn keyboard_note_event(&self, note: u8, on: bool) -> MixerEvent {
        let control = if on {
            ControlEvent::NoteOn {
                note,
                velocity: KEYBOARD_VELOCITY,
            }
        } else {
            ControlEvent::NoteOff { note }
        };
        to_instance(self.current_instance, InstanceEvent::Engine(control))
    }
}

pub(crate) fn print_help() {
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
    println!(
        "  sawvol|sawv <0..1>   squarevol|sqvol <0..1>   trianglevol|trivol <0..1>   sinevol|sinvol <0..1>"
    );
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

fn apply_parsed(session: &mut CommandSession, command: &ParsedCommand) {
    if let Some(instance) = command.switch_current {
        session.current_instance = instance;
    }
    for event in &command.events {
        session.apply_event(*event);
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
    Ok(n)
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

fn to_instance(instance: usize, event: InstanceEvent) -> MixerEvent {
    MixerEvent::ToInstance {
        instance: instance as u8,
        event,
    }
}

fn parse_line_commands(
    line: &str,
    current_instance: usize,
) -> Result<Option<ParsedCommand>, String> {
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
        let instance = parse_engine_number(tokens[1])?;
        if tokens.len() == 2 {
            return Ok(Some(ParsedCommand {
                switch_current: Some(instance),
                events: Vec::new(),
                print: PrintAfter::Status,
            }));
        }
        let rest = tokens[2..].join(" ");
        return parse_targeted_command(&rest, instance).map(Some);
    }

    parse_targeted_command(line, current_instance).map(Some)
}

fn parse_targeted_command(line: &str, instance: usize) -> Result<ParsedCommand, String> {
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
                events: vec![to_instance(
                    instance,
                    InstanceEvent::SetEnabled { on: true },
                )],
                print: PrintAfter::None,
            })
        }
        "off" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_instance(
                    instance,
                    InstanceEvent::SetEnabled { on: false },
                )],
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
                events: vec![to_instance(
                    instance,
                    InstanceEvent::SetListenChannel { channel },
                )],
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
                events: vec![to_instance(instance, InstanceEvent::SetVolume { amount })],
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
                print: PrintAfter::Show { instance: instance },
            })
        }
        "random" => {
            if parts.next().is_some() {
                return Err("too many arguments".to_string());
            }
            Ok(generate_random_patch(&mut rand::thread_rng(), instance))
        }
        _ => {
            let event =
                parse_param_command(line)?.ok_or_else(|| "expected a command".to_string())?;
            Ok(ParsedCommand {
                switch_current: None,
                events: vec![to_instance(instance, InstanceEvent::Engine(event))],
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

fn env3_dest_name(dest: AssignableDest) -> &'static str {
    match dest {
        AssignableDest::Off => "off",
        AssignableDest::Resonance => "res",
        AssignableDest::Pitch => "pitch",
        AssignableDest::Cutoff => "cutoff",
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
        params.amp_env.attack_ms,
        params.amp_env.decay_ms,
        params.amp_env.sustain,
        params.amp_env.release_ms,
        params.filter_env_amount,
        params.filter_env.attack_ms,
        params.filter_env.decay_ms,
        params.filter_env.sustain,
        params.filter_env.release_ms,
        env3_dest_name(params.assignable_dest),
        params.assignable_amount,
        params.assignable_env.attack_ms,
        params.assignable_env.decay_ms,
        params.assignable_env.sustain,
        params.assignable_env.release_ms,
        params.env_vel,
    )
}

fn format_eng_status(session: &CommandSession) -> String {
    let n = session.current_instance;
    let shadow = &session.shadows[n - 1];
    let state = if shadow.enabled { "on" } else { "off" };
    format!(
        "{}{}{}",
        qualified(n, state),
        qualified(n, &format!("ch {}", shadow.listen_channel)),
        qualified(n, &format!("vol {:.2}", shadow.volume)),
    )
}

fn format_show(session: &CommandSession, instance: usize) -> String {
    let n = instance;
    let shadow = &session.shadows[instance - 1];
    let state = if shadow.enabled { "on" } else { "off" };
    let mut out = String::new();
    out.push_str(&qualified(n, state));
    out.push_str(&qualified(n, &format!("ch {}", shadow.listen_channel)));
    out.push_str(&qualified(n, &format!("vol {:.2}", shadow.volume)));
    out.push_str(&qualify_block(n, &format_param_lines(&shadow.params)));
    out
}

fn wrap_engine(instance: usize, event: ControlEvent) -> MixerEvent {
    to_instance(instance, InstanceEvent::Engine(event))
}

fn generate_random_patch<R: Rng>(rng: &mut R, instance: usize) -> ParsedCommand {
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
    let env3_dest = RANDOM_ASSIGNABLE_DESTS[rng.gen_range(0..RANDOM_ASSIGNABLE_DESTS.len())];
    let env3_amt = match env3_dest {
        AssignableDest::Resonance => rng.gen_range(RANDOM_RES_AMT_MIN..=RANDOM_RES_AMT_MAX),
        AssignableDest::Off | AssignableDest::Pitch | AssignableDest::Cutoff => {
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
        amp_env: amp,
        filter_env,
        assignable_env: assign_env,
        filter_env_amount: filtenv_amt,
        assignable_amount: env3_amt,
        assignable_dest: env3_dest,
        env_link,
        env_vel: envvel,
        sub_vol,
        sub_octaves,
    };
    let n = instance;
    let mut report = qualified(n, &format!("vol {volume:.2}"));
    report.push_str(&qualify_block(n, &format_param_lines(&params)));

    let mut events = vec![
        wrap_engine(instance, ControlEvent::SetSawVol { amount: saw_vol }),
        wrap_engine(instance, ControlEvent::SetSquareVol { amount: square_vol }),
        wrap_engine(
            instance,
            ControlEvent::SetTriangleVol {
                amount: triangle_vol,
            },
        ),
        wrap_engine(instance, ControlEvent::SetSineVol { amount: sine_vol }),
        wrap_engine(instance, ControlEvent::SetPulse { width: pulse_width }),
        wrap_engine(instance, ControlEvent::SetSubVol { amount: sub_vol }),
        wrap_engine(
            instance,
            ControlEvent::SetSubOct {
                octaves: sub_octaves,
            },
        ),
        wrap_engine(instance, ControlEvent::SetCutoff { hz: cutoff_hz }),
        wrap_engine(instance, ControlEvent::SetResonance { amount: resonance }),
        wrap_engine(
            instance,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Amp,
                times: amp,
            },
        ),
        wrap_engine(
            instance,
            ControlEvent::SetFilterEnvAmount {
                amount: filtenv_amt,
            },
        ),
        wrap_engine(
            instance,
            ControlEvent::SetAssignableDest { dest: env3_dest },
        ),
        wrap_engine(
            instance,
            ControlEvent::SetAssignableAmount { amount: env3_amt },
        ),
        wrap_engine(instance, ControlEvent::SetEnvVel { amount: envvel }),
        to_instance(instance, InstanceEvent::SetVolume { amount: volume }),
    ];

    if env_link {
        events.push(wrap_engine(instance, ControlEvent::SetEnvLink { on: true }));
    } else {
        events.push(wrap_engine(
            instance,
            ControlEvent::SetEnvLink { on: false },
        ));
        events.push(wrap_engine(
            instance,
            ControlEvent::SetEnvelope {
                which: EnvelopeId::Filter,
                times: filter_env,
            },
        ));
        events.push(wrap_engine(
            instance,
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
            let name =
                arg.ok_or_else(|| "wave needs saw, square, triangle, or sine".to_string())?;
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
            Ok(Some(ControlEvent::SetFilterEnvAmount { amount }))
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
                "off" => AssignableDest::Off,
                "res" | "resonance" => AssignableDest::Resonance,
                "pitch" => AssignableDest::Pitch,
                "cutoff" => AssignableDest::Cutoff,
                other => {
                    return Err(format!(
                        "unknown dest '{other}' (use off, res, pitch, or cutoff)"
                    ));
                }
            };
            Ok(Some(ControlEvent::SetAssignableDest { dest }))
        }
        "env3amt" => {
            let amount = parse_f32_arg(arg, "env3amt")?;
            Ok(Some(ControlEvent::SetAssignableAmount { amount }))
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
            Some(ControlEvent::SetFilterEnvAmount { amount }) => {
                assert!((amount + 2.5).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_env3dest_tokens_and_alias() {
        match parse_param_command("env3dest off").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Off,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest res").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Resonance,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest resonance").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Resonance,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest pitch").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Pitch,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env3dest cutoff").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Cutoff,
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
        assert!(parse_line_commands("random extra", 1).is_err());
    }

    #[test]
    fn parses_random_command() {
        let parsed = parse_line_commands("random", 1)
            .unwrap()
            .expect("random should produce events");
        assert!(parsed.events.len() > 1);
        assert!(matches!(parsed.print, PrintAfter::Report(_)));
    }

    #[test]
    fn random_command_prints_a_replayable_patch() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let patch = generate_random_patch(&mut rng, 1);
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
            let patch = generate_random_patch(&mut rng, 2);
            let mut saw_link_on = false;
            let mut saw_link_off = false;
            let mut extra_env_times = 0usize;
            let mut env3_dest = AssignableDest::Off;
            let mut saw_volume = false;

            let mut saw_osc_levels = 0usize;
            for event in &patch.events {
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
                                assert!(
                                    *amount >= RANDOM_VOL_MIN && *amount <= RANDOM_VOL_MAX,
                                    "seed {seed}: volume {amount} out of range"
                                );
                                saw_volume = true;
                            }
                            InstanceEvent::Engine(control) => match control {
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
                                ControlEvent::SetFilterEnvAmount { amount } => {
                                    assert!(*amount >= RANDOM_AMT_MIN && *amount <= RANDOM_AMT_MAX);
                                }
                                ControlEvent::SetAssignableDest { dest } => env3_dest = *dest,
                                ControlEvent::SetAssignableAmount { amount } => match env3_dest {
                                    AssignableDest::Resonance => {
                                        assert!(
                                            *amount >= RANDOM_RES_AMT_MIN
                                                && *amount <= RANDOM_RES_AMT_MAX
                                        );
                                    }
                                    AssignableDest::Off
                                    | AssignableDest::Pitch
                                    | AssignableDest::Cutoff => {
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
        let mut session = CommandSession::new();
        assert_eq!(session.current_instance, 1);
        let parsed = parse_line_commands("eng 2 cutoff 800", session.current_instance)
            .unwrap()
            .expect("command");
        assert!(parsed.switch_current.is_none());
        match parsed.events.as_slice() {
            [
                MixerEvent::ToInstance {
                    instance: 2,
                    event: InstanceEvent::Engine(ControlEvent::SetCutoff { hz }),
                },
            ] => assert!((*hz - 800.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        apply_parsed(&mut session, &parsed);
        assert_eq!(session.current_instance, 1);
        assert!((session.shadows[1].params.cutoff_hz - 800.0).abs() < f32::EPSILON);
    }

    #[test]
    fn glued_eng2_errors() {
        assert!(parse_line_commands("eng2", 1).is_err());
        assert!(parse_line_commands("eng2 cutoff 800", 1).is_err());
    }

    #[test]
    fn eng_out_of_range_errors() {
        assert!(parse_line_commands("eng 0", 1).is_err());
        assert!(parse_line_commands("eng 5", 1).is_err());
    }

    #[test]
    fn ch_out_of_range_errors() {
        assert!(parse_line_commands("ch 0", 1).is_err());
        assert!(parse_line_commands("ch 17", 1).is_err());
    }

    #[test]
    fn eng_2_random_does_not_switch_current_or_routing() {
        let mut session = CommandSession::new();
        let parsed = parse_line_commands("eng 2 random", 1)
            .unwrap()
            .expect("random");
        assert!(parsed.switch_current.is_none());
        apply_parsed(&mut session, &parsed);
        assert_eq!(session.current_instance, 1);
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
        let mut session = CommandSession::new();
        let parsed = parse_line_commands("subvol 0.35", 1)
            .unwrap()
            .expect("subvol");
        apply_parsed(&mut session, &parsed);
        let parsed = parse_line_commands("suboct 2", 1).unwrap().expect("suboct");
        apply_parsed(&mut session, &parsed);
        let shown = format_show(&session, 1);
        assert!(shown.contains("subvol 0.35"));
        assert!(shown.contains("suboct 2"));
        assert!((session.shadows[0].params.sub_vol - 0.35).abs() < f32::EPSILON);
        assert_eq!(session.shadows[0].params.sub_octaves, SubOctaves::Two);
    }
}
