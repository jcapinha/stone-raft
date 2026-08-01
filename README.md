# stone-raft

A portable hardware synthesizer built on a Daisy Seed, written in Rust as a first Rust project. See [`CONTEXT.md`](CONTEXT.md) for the full project description.

## Project context

Living project truth for humans and agents:

- [`CONTEXT.md`](CONTEXT.md) — current project intent, domain language, and decisions
- [`REJECTED.md`](REJECTED.md) — closed doors (do not re-suggest these)

To stress-test a plan and update those files as you decide, use `/grill-with-docs` in this repo (project skill only; separate from personal `/grill-me`).

## Workspace layout

- [`engine/`](engine) — the sound-generation code (oscillators, filters, envelopes). Written `no_std` so the same code can run on the Daisy Seed later, with no operating system underneath.
- [`host-laptop/`](host-laptop) — a thin adapter that connects `engine` to your laptop's speakers ([cpal](https://docs.rs/cpal)) and MIDI ([midir](https://docs.rs/midir)). Used for fast development before porting to the Daisy.

## Running `host-laptop`

You need a Rust toolchain (install via [rustup](https://rustup.rs)).

### WSL / Linux

`cpal`'s Linux backend needs ALSA's development files at build time:

```bash
sudo apt update && sudo apt install -y pkg-config libasound2-dev
cargo run -p host-laptop
```

On WSL, audio is routed to Windows through WSLg's PulseAudio bridge, so you should hear sound through your normal Windows speakers.

### PowerShell (native Windows)

Not set up. Building `host-laptop` on Windows needs either a full MinGW installation (rustup's GNU toolchain ships `dlltool.exe` but not the assembler it calls, so the `windows-*` crates fail to build) or the MSVC toolchain with Visual Studio Build Tools. Neither is installed, so use WSL.

This is the only thing WSL cannot do: MIDI devices (WSL2 has no ALSA sequencer) and keyboard hold-to-play (see below).

### Playing notes

If a MIDI input port is available, the host connects to the **first** one and prints its name. Play notes from that device (all MIDI channels drive the single engine for now).

If MIDI cannot be opened (no ports, or no ALSA sequencer under WSL), use the laptop keyboard (one octave from C4):

| Keys | Notes |
|------|--------|
| `A` `W` `S` `E` `D` `F` `T` `G` `Y` `H` `U` `J` `K` | C C# D D# E F F# G G# A A# B C |

Press `q` to quit (in keyboard mode, just `q`; with MIDI open, type `q` then Enter).

### Live engine params (terminal)

While audio is running, change subtractive params with named line commands (no MIDI device required):

| Command | Meaning |
|---------|---------|
| `cutoff <Hz>` | Filter cutoff, e.g. `cutoff 800` |
| `res <0..1>` | Filter resonance, e.g. `res 0.3` |
| `attack <ms>` | Amp envelope attack time |
| `decay <ms>` | Amp envelope decay time |
| `sustain <0..1>` | Amp envelope sustain level |
| `release <ms>` | Amp envelope release time |
| `wave saw` / `wave square` | Oscillator shape for all voices |

In **keyboard mode**, press `/` to enter one command line, type the command, then Enter. In **MIDI mode** (line input already active), type the command on its own line, or `q` to quit.

Note release depends on the terminal. Most Unix terminals, including Windows Terminal running WSL, never report that a key came back up, so notes rely on the amp release stage and 4-voice stealing rather than true key-up. The native Windows console does report key release, but the Windows build is not set up (see above).

## Running `engine`'s tests

```bash
cargo test -p engine
```
