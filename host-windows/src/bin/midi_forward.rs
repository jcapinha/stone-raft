//! Copy note on/off from one MIDI input to the Daisy USB MIDI port.
//!
//! Run this from PowerShell, not from WSL. The `stone-raft` port exists only while
//! `bench-play` is running on the Seed. On quit, held notes get a note-off so a
//! stuck note does not depend on a CC the Daisy ignores.

use std::collections::HashSet;
use std::error::Error;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputConnection};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("stone-raft midi-forward")?;
    let in_ports = midi_in.ports();
    if in_ports.is_empty() {
        return Err("no MIDI input ports".into());
    }
    let in_port = select_input(&midi_in, &in_ports)?;
    let in_name = midi_in.port_name(&in_port)?;

    let midi_out = MidiOutput::new("stone-raft midi-forward")?;
    let out_ports = midi_out.ports();
    let mut listed = String::new();
    let mut chosen = None;
    for (index, port) in out_ports.iter().enumerate() {
        let name = midi_out
            .port_name(port)
            .unwrap_or_else(|_| "<unknown>".to_string());
        if chosen.is_none() && name.contains("stone-raft") {
            chosen = Some(index);
        }
        listed.push_str(&format!("  {index}: {name}\n"));
    }
    let Some(out_index) = chosen else {
        eprintln!("No MIDI output port contains \"stone-raft\". Is bench-play running?");
        eprintln!("MIDI output ports:");
        eprint!("{listed}");
        return Err("stone-raft MIDI output not found".into());
    };
    let out_port = &out_ports[out_index];
    let out_name = midi_out.port_name(out_port)?;
    let mut output = midi_out.connect(out_port, "stone-raft")?;

    let (tx, rx) = mpsc::sync_channel::<[u8; 3]>(256);
    let _input = midi_in.connect(
        &in_port,
        "stone-raft-forward",
        move |_stamp, message, _| {
            if let Some(bytes) = note_bytes(message) {
                let _ = tx.try_send(bytes);
            }
        },
        (),
    )?;

    println!("MIDI input: {in_name}");
    println!("MIDI output: {out_name}");
    println!("Forwarding note on/off. Channel stays as sent. Press Ctrl+C to quit.");

    let quit = Arc::new(AtomicBool::new(false));
    let quit_flag = Arc::clone(&quit);
    ctrlc::set_handler(move || {
        quit_flag.store(true, Ordering::SeqCst);
    })?;

    let mut held: HashSet<(u8, u8)> = HashSet::new();
    loop {
        if quit.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(bytes) => forward_note(&mut output, &mut held, bytes)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    release_held(&mut output, &held)?;
    Ok(())
}

fn select_input(
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

    let index = prompt_index(ports.len())?;
    Ok(ports[index].clone())
}

fn prompt_index(count: usize) -> Result<usize, Box<dyn Error>> {
    loop {
        print!("Select MIDI input number (0-{}): ", count - 1);
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if line.is_empty() {
            return Err("stdin closed before a MIDI input was chosen".into());
        }
        match line.trim().parse::<usize>() {
            Ok(index) if index < count => return Ok(index),
            _ => println!("Enter a number between 0 and {}.", count - 1),
        }
    }
}

fn note_bytes(message: &[u8]) -> Option<[u8; 3]> {
    if message.len() < 2 {
        return None;
    }
    let status = message[0];
    let kind = status & 0xF0;
    if kind != 0x80 && kind != 0x90 {
        return None;
    }
    Some([status, message[1], message.get(2).copied().unwrap_or(0)])
}

fn forward_note(
    output: &mut MidiOutputConnection,
    held: &mut HashSet<(u8, u8)>,
    bytes: [u8; 3],
) -> Result<(), Box<dyn Error>> {
    let channel = bytes[0] & 0x0F;
    let note = bytes[1];
    let velocity = bytes[2];
    let kind = bytes[0] & 0xF0;
    if kind == 0x90 && velocity > 0 {
        held.insert((channel, note));
    } else {
        held.remove(&(channel, note));
    }
    output.send(&bytes)?;
    Ok(())
}

fn release_held(
    output: &mut MidiOutputConnection,
    held: &HashSet<(u8, u8)>,
) -> Result<(), Box<dyn Error>> {
    for &(channel, note) in held {
        output.send(&[0x80 | channel, note, 0])?;
    }
    if !held.is_empty() {
        println!("Sent note-off for {} held note(s).", held.len());
    }
    Ok(())
}
