//! Terminal command parsing and host-side engine state.

use engine::{
    AdsrTimes, AssignableDest, ControlEvent, EngineParams, EnvelopeField, EnvelopeId,
    InstanceEvent, LfoId, LfoParams, LfoWave, MixerEvent, SubOctaves, Waveform, ENGINE_COUNT,
    LFO_RATE_MAX_HZ, LFO_RATE_MIN_HZ,
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
const RANDOM_PW_AMT_MIN: f32 = -0.4;
const RANDOM_PW_AMT_MAX: f32 = 0.4;
const RANDOM_AMP_AMT_MIN: f32 = -0.8;
const RANDOM_AMP_AMT_MAX: f32 = 0.8;
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
    println!("  saw|sq|tri|sin|sub <0..1>");
    println!("  wave saw|square|triangle|sine   pw <0.05..0.95>   suboct 1|2");
    println!("  amp a|d|r <ms>   amp s <0..1>");
    println!("  fenv a|d|r <ms>   fenv s <0..1>   fenv amt <signed octaves>");
    println!("  asenv a|d|r <ms>   asenv s <0..1>");
    println!("  asenv dest off|res|pitch|cutoff|pw|amp   asenv amt <signed>");
    println!("  env copy   env link on|off   env vel <0..1>");
    println!("  lfo 1|2 dest off|res|pitch|cutoff|pw|amp   lfo 1|2 amt <signed>");
    println!(
        "  lfo 1|2 rate <0.05..20>   lfo 1|2 wave sine|tri|square|saw|sh   lfo 1|2 retrig on|off"
    );
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

fn is_glued_lfo_token(cmd: &str) -> bool {
    match cmd.strip_prefix("lfo") {
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
    if is_glued_lfo_token(&first) {
        return Err("use 'lfo N' with a space (for example lfo 1)".to_string());
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
    if is_glued_lfo_token(&cmd) {
        return Err("use 'lfo N' with a space (for example lfo 1)".to_string());
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

fn assignable_dest_name(dest: AssignableDest) -> &'static str {
    match dest {
        AssignableDest::Off => "off",
        AssignableDest::Resonance => "res",
        AssignableDest::Pitch => "pitch",
        AssignableDest::Cutoff => "cutoff",
        AssignableDest::PulseWidth => "pw",
        AssignableDest::Amp => "amp",
    }
}

fn lfo_wave_name(wave: LfoWave) -> &'static str {
    match wave {
        LfoWave::Sine => "sine",
        LfoWave::Triangle => "tri",
        LfoWave::Square => "square",
        LfoWave::Saw => "saw",
        LfoWave::SampleHold => "sh",
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

fn format_lfo_lines(which: usize, lfo: &LfoParams) -> String {
    let retrig = if lfo.retrigger { "on" } else { "off" };
    format!(
        "lfo {which} dest {}\n\
         lfo {which} amt {:.2}\n\
         lfo {which} rate {:.2}\n\
         lfo {which} wave {}\n\
         lfo {which} retrig {retrig}\n",
        assignable_dest_name(lfo.dest),
        lfo.amount,
        lfo.rate_hz,
        lfo_wave_name(lfo.wave),
    )
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
    let mut out = format!(
        "saw {:.2}\n\
         sq {:.2}\n\
         tri {:.2}\n\
         sin {:.2}\n\
         pw {:.2}\n\
         sub {:.2}\n\
         suboct {}\n\
         cutoff {:.0}\n\
         res {:.2}\n\
         amp a {:.0}\n\
         amp d {:.0}\n\
         amp s {:.2}\n\
         amp r {:.0}\n\
         fenv amt {:.2}\n\
         fenv a {:.0}\n\
         fenv d {:.0}\n\
         fenv s {:.2}\n\
         fenv r {:.0}\n\
         asenv dest {}\n\
         asenv amt {:.2}\n\
         asenv a {:.0}\n\
         asenv d {:.0}\n\
         asenv s {:.2}\n\
         asenv r {:.0}\n\
         env vel {:.2}\n\
         env link {link_name}\n",
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
        assignable_dest_name(params.assignable_dest),
        params.assignable_amount,
        params.assignable_env.attack_ms,
        params.assignable_env.decay_ms,
        params.assignable_env.sustain,
        params.assignable_env.release_ms,
        params.env_vel,
    );
    out.push_str(&format_lfo_lines(1, &params.lfos[0]));
    out.push_str(&format_lfo_lines(2, &params.lfos[1]));
    out
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
    let filter_env_amount = rng.gen_range(RANDOM_AMT_MIN..=RANDOM_AMT_MAX);
    let assignable_dest = RANDOM_ASSIGNABLE_DESTS[rng.gen_range(0..RANDOM_ASSIGNABLE_DESTS.len())];
    let assignable_amount = random_amount_for_dest(rng, assignable_dest);
    let lfos = [random_lfo(rng), random_lfo(rng)];
    let env_vel = rng.gen_range(0.0..=1.0);
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
        filter_env_amount,
        assignable_amount,
        assignable_dest,
        env_link,
        env_vel,
        sub_vol,
        sub_octaves,
        lfos,
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
                amount: filter_env_amount,
            },
        ),
        wrap_engine(
            instance,
            ControlEvent::SetAssignableDest {
                dest: assignable_dest,
            },
        ),
        wrap_engine(
            instance,
            ControlEvent::SetAssignableAmount {
                amount: assignable_amount,
            },
        ),
        wrap_engine(instance, ControlEvent::SetEnvVel { amount: env_vel }),
        to_instance(instance, InstanceEvent::SetVolume { amount: volume }),
    ];
    for which in [LfoId::One, LfoId::Two] {
        let lfo = lfos[which.index()];
        events.push(wrap_engine(
            instance,
            ControlEvent::SetLfoDest {
                which,
                dest: lfo.dest,
            },
        ));
        events.push(wrap_engine(
            instance,
            ControlEvent::SetLfoAmount {
                which,
                amount: lfo.amount,
            },
        ));
        events.push(wrap_engine(
            instance,
            ControlEvent::SetLfoRate {
                which,
                rate_hz: lfo.rate_hz,
            },
        ));
        events.push(wrap_engine(
            instance,
            ControlEvent::SetLfoWave {
                which,
                wave: lfo.wave,
            },
        ));
        events.push(wrap_engine(
            instance,
            ControlEvent::SetLfoRetrig {
                which,
                on: lfo.retrigger,
            },
        ));
    }

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
    let args: Vec<&str> = parts.collect();

    match cmd.as_str() {
        "cutoff" => {
            let hz = parse_single_f32_arg(&args, "cutoff")?;
            Ok(Some(ControlEvent::SetCutoff { hz }))
        }
        "res" | "resonance" => {
            let amount = parse_single_f32_arg(&args, "res")?;
            Ok(Some(ControlEvent::SetResonance { amount }))
        }
        "amp" => parse_grouped_envelope(&args, EnvelopeId::Amp, "amp"),
        "attack" => envelope_patch(
            EnvelopeId::Amp,
            EnvelopeField::Attack,
            single_optional_arg(&args)?,
            "attack",
        ),
        "decay" => envelope_patch(
            EnvelopeId::Amp,
            EnvelopeField::Decay,
            single_optional_arg(&args)?,
            "decay",
        ),
        "sustain" => envelope_patch(
            EnvelopeId::Amp,
            EnvelopeField::Sustain,
            single_optional_arg(&args)?,
            "sustain",
        ),
        "release" => envelope_patch(
            EnvelopeId::Amp,
            EnvelopeField::Release,
            single_optional_arg(&args)?,
            "release",
        ),
        "saw" | "sawvol" | "sawv" => {
            let amount = parse_single_f32_arg(&args, "saw")?;
            Ok(Some(ControlEvent::SetSawVol { amount }))
        }
        "sq" | "squarevol" | "sqvol" => {
            let amount = parse_single_f32_arg(&args, "sq")?;
            Ok(Some(ControlEvent::SetSquareVol { amount }))
        }
        "tri" | "trianglevol" | "trivol" => {
            let amount = parse_single_f32_arg(&args, "tri")?;
            Ok(Some(ControlEvent::SetTriangleVol { amount }))
        }
        "sin" | "sinevol" | "sinvol" => {
            let amount = parse_single_f32_arg(&args, "sin")?;
            Ok(Some(ControlEvent::SetSineVol { amount }))
        }
        "wave" => {
            let name = single_required_arg(&args, "wave needs saw, square, triangle, or sine")?;
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
        "pw" | "pulse" => {
            let width = parse_single_f32_arg(&args, "pw")?;
            Ok(Some(ControlEvent::SetPulse { width }))
        }
        "sub" | "subvol" => {
            let amount = parse_single_f32_arg(&args, "sub")?;
            Ok(Some(ControlEvent::SetSubVol { amount }))
        }
        "suboct" => {
            let raw = single_required_arg(&args, "suboct needs 1 or 2")?;
            let octaves = match raw {
                "1" => SubOctaves::One,
                "2" => SubOctaves::Two,
                other => {
                    return Err(format!("unknown suboct '{other}' (use 1 or 2)"));
                }
            };
            Ok(Some(ControlEvent::SetSubOct { octaves }))
        }
        "fenv" => parse_filter_envelope_command(&args),
        "filtenvamt" => {
            let amount = parse_single_f32_arg(&args, "filtenvamt")?;
            Ok(Some(ControlEvent::SetFilterEnvAmount { amount }))
        }
        "filtenvattack" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Attack,
            single_optional_arg(&args)?,
            "filtenvattack",
        ),
        "filtenvdecay" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Decay,
            single_optional_arg(&args)?,
            "filtenvdecay",
        ),
        "filtenvsustain" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Sustain,
            single_optional_arg(&args)?,
            "filtenvsustain",
        ),
        "filtenvrelease" => envelope_patch(
            EnvelopeId::Filter,
            EnvelopeField::Release,
            single_optional_arg(&args)?,
            "filtenvrelease",
        ),
        "asenv" => parse_assignable_envelope_command(&args),
        "env3dest" => {
            let name =
                single_required_arg(&args, "env3dest needs off, res, pitch, cutoff, pw, or amp")?;
            let dest = parse_assignable_dest(name)?;
            Ok(Some(ControlEvent::SetAssignableDest { dest }))
        }
        "env3amt" => {
            let amount = parse_single_f32_arg(&args, "env3amt")?;
            Ok(Some(ControlEvent::SetAssignableAmount { amount }))
        }
        "env3attack" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Attack,
            single_optional_arg(&args)?,
            "env3attack",
        ),
        "env3decay" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Decay,
            single_optional_arg(&args)?,
            "env3decay",
        ),
        "env3sustain" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Sustain,
            single_optional_arg(&args)?,
            "env3sustain",
        ),
        "env3release" => envelope_patch(
            EnvelopeId::Assignable,
            EnvelopeField::Release,
            single_optional_arg(&args)?,
            "env3release",
        ),
        "lfo" => parse_lfo_command(&args),
        "env" => parse_shared_envelope_command(&args),
        "envcopy" => {
            expect_no_args(&args)?;
            Ok(Some(ControlEvent::EnvCopy))
        }
        "envlink" => {
            let name = single_required_arg(&args, "envlink needs on or off")?;
            let on = parse_on_off(name, "envlink")?;
            Ok(Some(ControlEvent::SetEnvLink { on }))
        }
        "envvel" => {
            let amount = parse_single_f32_arg(&args, "envvel")?;
            Ok(Some(ControlEvent::SetEnvVel { amount }))
        }
        other => Err(format!(
            "unknown command '{other}' (eng, on, off, ch, vol, show, cutoff, res, saw, sq, tri, sin, sub, wave, pw, suboct, amp, fenv, asenv, env, lfo, random)"
        )),
    }
}

fn parse_grouped_envelope(
    args: &[&str],
    which: EnvelopeId,
    group: &str,
) -> Result<Option<ControlEvent>, String> {
    if args.len() < 2 {
        return Err(format!("{group} needs a, d, s, or r followed by a number"));
    }
    if args.len() > 2 {
        return Err("too many arguments".to_string());
    }
    let field = match args[0].to_ascii_lowercase().as_str() {
        "a" => EnvelopeField::Attack,
        "d" => EnvelopeField::Decay,
        "s" => EnvelopeField::Sustain,
        "r" => EnvelopeField::Release,
        other => {
            return Err(format!(
                "unknown {group} field '{other}' (use a, d, s, or r)"
            ));
        }
    };
    let value = args[1].parse::<f32>().map_err(|_| {
        format!(
            "could not parse '{}' as a number for {group} {}",
            args[1], args[0]
        )
    })?;
    Ok(Some(ControlEvent::PatchEnvelope {
        which,
        field,
        value,
    }))
}

fn parse_filter_envelope_command(args: &[&str]) -> Result<Option<ControlEvent>, String> {
    if args
        .first()
        .is_some_and(|field| field.eq_ignore_ascii_case("amt"))
    {
        let amount = parse_grouped_f32_arg(args, "fenv amt")?;
        return Ok(Some(ControlEvent::SetFilterEnvAmount { amount }));
    }
    parse_grouped_envelope(args, EnvelopeId::Filter, "fenv")
}

fn parse_assignable_envelope_command(args: &[&str]) -> Result<Option<ControlEvent>, String> {
    match args.first().map(|part| part.to_ascii_lowercase()) {
        Some(field) if field == "amt" => {
            let amount = parse_grouped_f32_arg(args, "asenv amt")?;
            Ok(Some(ControlEvent::SetAssignableAmount { amount }))
        }
        Some(field) if field == "dest" => {
            if args.len() < 2 {
                return Err("asenv dest needs off, res, pitch, cutoff, pw, or amp".to_string());
            }
            if args.len() > 2 {
                return Err("too many arguments".to_string());
            }
            let dest = parse_assignable_dest(args[1])?;
            Ok(Some(ControlEvent::SetAssignableDest { dest }))
        }
        _ => parse_grouped_envelope(args, EnvelopeId::Assignable, "asenv"),
    }
}

fn parse_lfo_id(raw: &str) -> Result<LfoId, String> {
    match raw {
        "1" => Ok(LfoId::One),
        "2" => Ok(LfoId::Two),
        _ => Err("lfo needs 1 or 2".to_string()),
    }
}

fn parse_lfo_wave(name: &str) -> Result<LfoWave, String> {
    match name.to_ascii_lowercase().as_str() {
        "sine" => Ok(LfoWave::Sine),
        "tri" | "triangle" => Ok(LfoWave::Triangle),
        "square" | "sq" => Ok(LfoWave::Square),
        "saw" => Ok(LfoWave::Saw),
        "sh" | "snh" => Ok(LfoWave::SampleHold),
        other => Err(format!(
            "unknown wave '{other}' (use sine, tri, square, saw, or sh)"
        )),
    }
}

fn parse_lfo_command(args: &[&str]) -> Result<Option<ControlEvent>, String> {
    if args.len() < 3 {
        return Err("lfo needs 1 or 2, then dest, amt, rate, wave, or retrig".to_string());
    }
    let which = parse_lfo_id(args[0])?;
    match args[1].to_ascii_lowercase().as_str() {
        "dest" => {
            if args.len() > 3 {
                return Err("too many arguments".to_string());
            }
            let dest = parse_assignable_dest(args[2])?;
            Ok(Some(ControlEvent::SetLfoDest { which, dest }))
        }
        "amt" => {
            if args.len() > 3 {
                return Err("too many arguments".to_string());
            }
            let amount = args[2]
                .parse::<f32>()
                .map_err(|_| format!("could not parse '{}' as a number for lfo amt", args[2]))?;
            Ok(Some(ControlEvent::SetLfoAmount { which, amount }))
        }
        "rate" => {
            if args.len() > 3 {
                return Err("too many arguments".to_string());
            }
            let rate_hz = args[2]
                .parse::<f32>()
                .map_err(|_| format!("could not parse '{}' as a number for lfo rate", args[2]))?;
            Ok(Some(ControlEvent::SetLfoRate { which, rate_hz }))
        }
        "wave" => {
            if args.len() > 3 {
                return Err("too many arguments".to_string());
            }
            let wave = parse_lfo_wave(args[2])?;
            Ok(Some(ControlEvent::SetLfoWave { which, wave }))
        }
        "retrig" => {
            if args.len() > 3 {
                return Err("too many arguments".to_string());
            }
            let on = parse_on_off(args[2], "lfo retrig")?;
            Ok(Some(ControlEvent::SetLfoRetrig { which, on }))
        }
        other => Err(format!(
            "unknown lfo field '{other}' (use dest, amt, rate, wave, or retrig)"
        )),
    }
}

fn parse_shared_envelope_command(args: &[&str]) -> Result<Option<ControlEvent>, String> {
    match args.first().map(|part| part.to_ascii_lowercase()) {
        Some(action) if action == "copy" => {
            if args.len() > 1 {
                return Err("too many arguments".to_string());
            }
            Ok(Some(ControlEvent::EnvCopy))
        }
        Some(action) if action == "link" => {
            if args.len() < 2 {
                return Err("env link needs on or off".to_string());
            }
            if args.len() > 2 {
                return Err("too many arguments".to_string());
            }
            let on = parse_on_off(args[1], "env link")?;
            Ok(Some(ControlEvent::SetEnvLink { on }))
        }
        Some(action) if action == "vel" => {
            let amount = parse_grouped_f32_arg(args, "env vel")?;
            Ok(Some(ControlEvent::SetEnvVel { amount }))
        }
        Some(other) => Err(format!(
            "unknown env action '{other}' (use copy, link, or vel)"
        )),
        None => Err("env needs copy, link, or vel".to_string()),
    }
}

fn parse_assignable_dest(name: &str) -> Result<AssignableDest, String> {
    match name.to_ascii_lowercase().as_str() {
        "off" => Ok(AssignableDest::Off),
        "res" | "resonance" => Ok(AssignableDest::Resonance),
        "pitch" => Ok(AssignableDest::Pitch),
        "cutoff" => Ok(AssignableDest::Cutoff),
        "pw" | "pulse" | "pwm" => Ok(AssignableDest::PulseWidth),
        "amp" => Ok(AssignableDest::Amp),
        other => Err(format!(
            "unknown dest '{other}' (use off, res, pitch, cutoff, pw, or amp)"
        )),
    }
}

fn parse_on_off(name: &str, command: &str) -> Result<bool, String> {
    match name.to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("unknown {command} '{other}' (use on or off)")),
    }
}

fn parse_grouped_f32_arg(args: &[&str], name: &str) -> Result<f32, String> {
    if args.len() < 2 {
        return Err(format!("{name} needs a number"));
    }
    if args.len() > 2 {
        return Err("too many arguments".to_string());
    }
    args[1]
        .parse::<f32>()
        .map_err(|_| format!("could not parse '{}' as a number for {name}", args[1]))
}

fn parse_single_f32_arg(args: &[&str], name: &str) -> Result<f32, String> {
    parse_f32_arg(single_optional_arg(args)?, name)
}

fn single_required_arg<'a>(args: &'a [&str], missing: &str) -> Result<&'a str, String> {
    match args {
        [] => Err(missing.to_string()),
        [arg] => Ok(arg),
        _ => Err("too many arguments".to_string()),
    }
}

fn single_optional_arg<'a>(args: &'a [&str]) -> Result<Option<&'a str>, String> {
    match args {
        [] => Ok(None),
        [arg] => Ok(Some(arg)),
        _ => Err("too many arguments".to_string()),
    }
}

fn expect_no_args(args: &[&str]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("too many arguments".to_string())
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
        match parse_param_command("saw 0.6").unwrap() {
            Some(ControlEvent::SetSawVol { amount }) => {
                assert!((amount - 0.6).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sq 0.4").unwrap() {
            Some(ControlEvent::SetSquareVol { amount }) => {
                assert!((amount - 0.4).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("tri 0.3").unwrap() {
            Some(ControlEvent::SetTriangleVol { amount }) => {
                assert!((amount - 0.3).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sin 0.2").unwrap() {
            Some(ControlEvent::SetSineVol { amount }) => {
                assert!((amount - 0.2).abs() < f32::EPSILON)
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
        match parse_param_command("pw 0.35").unwrap() {
            Some(ControlEvent::SetPulse { width }) => {
                assert!((width - 0.35).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("subvol 0.4").unwrap() {
            Some(ControlEvent::SetSubVol { amount }) => {
                assert!((amount - 0.4).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("sub 0.5").unwrap() {
            Some(ControlEvent::SetSubVol { amount }) => {
                assert!((amount - 0.5).abs() < f32::EPSILON)
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
        match parse_param_command("amp a 15").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Amp,
                field: EnvelopeField::Attack,
                value,
            }) => assert!((value - 15.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("fenv s 0.4").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Filter,
                field: EnvelopeField::Sustain,
                value,
            }) => assert!((value - 0.4).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv r 120").unwrap() {
            Some(ControlEvent::PatchEnvelope {
                which: EnvelopeId::Assignable,
                field: EnvelopeField::Release,
                value,
            }) => assert!((value - 120.0).abs() < f32::EPSILON),
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
        match parse_param_command("fenv amt 1.5").unwrap() {
            Some(ControlEvent::SetFilterEnvAmount { amount }) => {
                assert!((amount - 1.5).abs() < f32::EPSILON)
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
        match parse_param_command("asenv dest pitch").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Pitch,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv dest pw").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::PulseWidth,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv dest pulse").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::PulseWidth,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv dest pwm").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::PulseWidth,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv dest amp").unwrap() {
            Some(ControlEvent::SetAssignableDest {
                dest: AssignableDest::Amp,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("asenv amt -1.25").unwrap() {
            Some(ControlEvent::SetAssignableAmount { amount }) => {
                assert!((amount + 1.25).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_env3dest() {
        let err = parse_param_command("env3dest foo").unwrap_err();
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
        match parse_param_command("env copy").unwrap() {
            Some(ControlEvent::EnvCopy) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env link on").unwrap() {
            Some(ControlEvent::SetEnvLink { on: true }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("env vel 0.75").unwrap() {
            Some(ControlEvent::SetEnvVel { amount }) => {
                assert!((amount - 0.75).abs() < f32::EPSILON)
            }
            other => panic!("unexpected {other:?}"),
        }
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
        assert!(report.contains("saw "));
        assert!(report.contains("sq "));
        assert!(report.contains("tri "));
        assert!(report.contains("sin "));
        assert!(report.contains("pw "));
        assert!(report.contains("sub "));
        assert!(report.contains("suboct "));
        assert!(report.contains("cutoff "));
        assert!(report.contains("env link "));
        assert!(report.contains("lfo 1 "));
        assert!(report.contains("lfo 2 "));
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
            let mut lfo_dests = [AssignableDest::Off; 2];
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
                                    AssignableDest::PulseWidth => {
                                        assert!(
                                            *amount >= RANDOM_PW_AMT_MIN
                                                && *amount <= RANDOM_PW_AMT_MAX
                                        );
                                    }
                                    AssignableDest::Amp => {
                                        assert!(
                                            *amount >= RANDOM_AMP_AMT_MIN
                                                && *amount <= RANDOM_AMP_AMT_MAX
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
                                ControlEvent::SetLfoDest { which, dest } => {
                                    lfo_dests[which.index()] = *dest;
                                }
                                ControlEvent::SetLfoAmount { which, amount } => {
                                    match lfo_dests[which.index()] {
                                        AssignableDest::Resonance => {
                                            assert!(
                                                *amount >= RANDOM_RES_AMT_MIN
                                                    && *amount <= RANDOM_RES_AMT_MAX
                                            );
                                        }
                                        AssignableDest::PulseWidth => {
                                            assert!(
                                                *amount >= RANDOM_PW_AMT_MIN
                                                    && *amount <= RANDOM_PW_AMT_MAX
                                            );
                                        }
                                        AssignableDest::Amp => {
                                            assert!(
                                                *amount >= RANDOM_AMP_AMT_MIN
                                                    && *amount <= RANDOM_AMP_AMT_MAX
                                            );
                                        }
                                        AssignableDest::Off
                                        | AssignableDest::Pitch
                                        | AssignableDest::Cutoff => {
                                            assert!(
                                                *amount >= RANDOM_AMT_MIN
                                                    && *amount <= RANDOM_AMT_MAX
                                            );
                                        }
                                    }
                                }
                                ControlEvent::SetLfoRate { rate_hz, .. } => {
                                    assert!(
                                        *rate_hz >= LFO_RATE_MIN_HZ && *rate_hz <= LFO_RATE_MAX_HZ,
                                        "seed {seed}: LFO rate {rate_hz} out of range"
                                    );
                                }
                                ControlEvent::SetLfoWave { .. }
                                | ControlEvent::SetLfoRetrig { .. } => {}
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
            assert!(report.contains("saw "));
            assert!(report.contains("sq "));
            assert!(report.contains("sub "));
            assert!(report.contains("suboct "));
            assert!(report.contains("lfo 1 "));
            assert!(report.contains("lfo 2 "));
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
            [MixerEvent::ToInstance {
                instance: 2,
                event: InstanceEvent::Engine(ControlEvent::SetCutoff { hz }),
            }] => assert!((*hz - 800.0).abs() < f32::EPSILON),
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
    fn parses_lfo_commands_and_aliases() {
        match parse_param_command("lfo 1 dest pitch").unwrap() {
            Some(ControlEvent::SetLfoDest {
                which: LfoId::One,
                dest: AssignableDest::Pitch,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 2 dest pw").unwrap() {
            Some(ControlEvent::SetLfoDest {
                which: LfoId::Two,
                dest: AssignableDest::PulseWidth,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 1 amt -0.25").unwrap() {
            Some(ControlEvent::SetLfoAmount {
                which: LfoId::One,
                amount,
            }) => assert!((amount + 0.25).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 2 rate 4").unwrap() {
            Some(ControlEvent::SetLfoRate {
                which: LfoId::Two,
                rate_hz,
            }) => assert!((rate_hz - 4.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 1 wave triangle").unwrap() {
            Some(ControlEvent::SetLfoWave {
                which: LfoId::One,
                wave: LfoWave::Triangle,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 1 wave sq").unwrap() {
            Some(ControlEvent::SetLfoWave {
                which: LfoId::One,
                wave: LfoWave::Square,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 2 wave snh").unwrap() {
            Some(ControlEvent::SetLfoWave {
                which: LfoId::Two,
                wave: LfoWave::SampleHold,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 1 retrig off").unwrap() {
            Some(ControlEvent::SetLfoRetrig {
                which: LfoId::One,
                on: false,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match parse_param_command("lfo 2 retrig on").unwrap() {
            Some(ControlEvent::SetLfoRetrig {
                which: LfoId::Two,
                on: true,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn glued_lfo1_errors() {
        match parse_line_commands("lfo1 dest pitch", 1) {
            Err(err) => assert!(err.contains("lfo N")),
            Ok(_) => panic!("glued lfo1 should error"),
        }
        assert!(parse_line_commands("lfo1", 1).is_err());
        assert!(parse_line_commands("eng 2 lfo1 rate 4", 1).is_err());
    }

    #[test]
    fn show_includes_lfo_lines() {
        let session = CommandSession::new();
        let shown = format_show(&session, 1);
        assert!(shown.contains("lfo 1 dest off"));
        assert!(shown.contains("lfo 1 amt 0.00"));
        assert!(shown.contains("lfo 1 rate 1.00"));
        assert!(shown.contains("lfo 1 wave sine"));
        assert!(shown.contains("lfo 1 retrig on"));
        assert!(shown.contains("lfo 2 dest off"));
        assert!(shown.contains("lfo 2 retrig on"));
    }

    #[test]
    fn eng_2_lfo_1_rate_is_oneshot() {
        let mut session = CommandSession::new();
        let parsed = parse_line_commands("eng 2 lfo 1 rate 4", session.current_instance)
            .unwrap()
            .expect("command");
        assert!(parsed.switch_current.is_none());
        match parsed.events.as_slice() {
            [MixerEvent::ToInstance {
                instance: 2,
                event:
                    InstanceEvent::Engine(ControlEvent::SetLfoRate {
                        which: LfoId::One,
                        rate_hz,
                    }),
            }] => assert!((*rate_hz - 4.0).abs() < f32::EPSILON),
            other => panic!("unexpected {other:?}"),
        }
        apply_parsed(&mut session, &parsed);
        assert_eq!(session.current_instance, 1);
        assert!((session.shadows[1].params.lfos[0].rate_hz - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn show_includes_sub_and_suboct() {
        let mut session = CommandSession::new();
        let parsed = parse_line_commands("sub 0.35", 1).unwrap().expect("sub");
        apply_parsed(&mut session, &parsed);
        let parsed = parse_line_commands("suboct 2", 1).unwrap().expect("suboct");
        apply_parsed(&mut session, &parsed);
        let shown = format_show(&session, 1);
        assert!(shown.contains("sub 0.35"));
        assert!(shown.contains("suboct 2"));
        assert!((session.shadows[0].params.sub_vol - 0.35).abs() < f32::EPSILON);
        assert_eq!(session.shadows[0].params.sub_octaves, SubOctaves::Two);
    }
}
