# stone-raft

A portable hardware synthesizer built on a Daisy Seed, written in Rust as a first Rust project. See [`CONTEXT.md`](CONTEXT.md) for the full project description.

## Project context

Living project truth for humans and agents:

- [`CONTEXT.md`](CONTEXT.md) — current project intent, domain language, and decisions
- [`REJECTED.md`](REJECTED.md) — closed doors (do not re-suggest these)

To stress-test a plan and update those files as you decide, use `/grill-with-docs` in this repo (project skill only; separate from personal `/grill-me`).

## Workspace layout

- [`engine/`](engine) — the sound-generation code (oscillators, filters, envelopes). Written `no_std` so the same code can run on the Daisy Seed later, with no operating system underneath.
- [`host-common/`](host-common) — shared laptop host plumbing (audio/MIDI wiring, param commands, keyboard fallback) used by both OS binaries.
- [`host-wsl/`](host-wsl) — thin WSL/Linux binary for development and optional quick listening (WSLg audio can be flaky).
- [`host-windows/`](host-windows) — thin native Windows binary for reliable play, MIDI devices, and keyboard hold-to-play (MSVC toolchain).

## Running on WSL (`host-wsl`)

You need a Rust toolchain (install via [rustup](https://rustup.rs)).

`cpal`'s Linux backend needs ALSA's development files at build time:

```bash
sudo apt update && sudo apt install -y pkg-config libasound2-dev
cargo run -p host-wsl
```

On WSL, audio is routed to Windows through WSLg's PulseAudio bridge. That path can drop the stream (ALSA I/O errors). Prefer `host-windows` when you want to hear and play properly.

WSL usually has no ALSA sequencer, so MIDI ports are often unavailable and the host falls back to the laptop keyboard.

## Running on Windows (`host-windows`)

Use PowerShell (or Windows Terminal) with the **MSVC** Rust toolchain. The GNU/MinGW path is not supported.

### One-time setup

Rust in WSL does **not** count here. `host-windows` needs a separate Windows install of rustup (MSVC).

1. Install Visual Studio Build Tools with the C++ workload:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

2. Install rustup for Windows (opens the official installer):

```powershell
winget install Rustlang.Rustup
```

Close and reopen PowerShell after this so `rustup` and `cargo` are on `PATH`. When the rustup installer asks for a default host triple, keep **`x86_64-pc-windows-msvc`** (the default).

3. Confirm the MSVC toolchain is the default:

```powershell
rustup default stable-x86_64-pc-windows-msvc
rustup show
```

4. Open this repo from Windows. If the files live in WSL, list distros under `\\wsl$` in Explorer or PowerShell, then:

```powershell
cd \\wsl$\<Distro>\home\capinha\audio_experiments\stone-raft
cargo run -p host-windows
```

Replace `<Distro>` with your WSL distro folder name (for example `Ubuntu`).

Building over `\\wsl$\...` often fails incremental compilation with an “Incorrect function” lock-file error. Keep the source on WSL, but put the build output on a real Windows drive:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo run -p host-windows
```

Those two `$env:...` lines only last for that PowerShell window. Run them again in each new session before `cargo run`.

### Playing notes

If there is exactly one MIDI input port, the host connects to it automatically. If there are several, it lists them and asks for a number. MIDI notes go to every enabled engine whose listen channel matches. Engine 1 starts on, listen channel 1. Engines 2–4 start off, with listen channels 2, 3, and 4 already set. Use `on` (or `eng 2 on`) before a disabled engine will sound.

If MIDI cannot be opened, use the laptop keyboard (one octave from C4). Keyboard notes play the current engine only when that engine is on. If it is off, the host prints `engine N is off; type: on`.

| Keys | Notes |
|------|--------|
| `A` `W` `S` `E` `D` `F` `T` `G` `Y` `H` `U` `J` `K` | C C# D D# E F F# G G# A A# B C |

Press `q` to quit (in keyboard mode, just `q`; with MIDI open, type `q` then Enter).

Audio outputs work the same way: one device is selected automatically; several devices means a numbered list to pick from.

### Live engine params (terminal)

While audio is running, change routing and subtractive params with named line commands (no MIDI device required). Unqualified commands hit the current engine. `eng 2 cutoff 800` is one-shot and does not change current. Space is required (`eng2` is an error). `show` prints a host-side copy; `random` fills subtractive params plus volume (0.2–1.0) and prints `eng N` lines including `vol`. Neither `random` nor `show` changes on/off or listen channel.

| Command | Meaning |
|---------|---------|
| `eng` | Print current engine on/off, listen channel, and volume (1-based) |
| `eng <1..4>` | Switch current engine and print that status |
| `on` / `off` | Enable or disable current (`off` silences immediately) |
| `ch <1..16>` | MIDI listen channel |
| `vol <0..1>` | Instance volume |
| `show` | Print a replayable qualified patch (`eng N ...` lines) |
| `cutoff <Hz>` | Filter cutoff, e.g. `cutoff 800` |
| `res <0..1>` | Filter resonance, e.g. `res 0.3` |
| `attack <ms>` | Amp envelope attack time |
| `decay <ms>` | Amp envelope decay time |
| `sustain <0..1>` | Amp envelope sustain level |
| `release <ms>` | Amp envelope release time |
| `wave saw` / `wave square` | Oscillator shape for all voices |
| `filtenvamt <signed>` | Filter envelope amount in octaves, e.g. `filtenvamt 3` or `filtenvamt -2` |
| `filtenvattack <ms>` | Filter envelope attack time |
| `filtenvdecay <ms>` | Filter envelope decay time |
| `filtenvsustain <0..1>` | Filter envelope sustain level |
| `filtenvrelease <ms>` | Filter envelope release time |
| `env3dest off` / `res` / `pitch` / `cutoff` | Assignable envelope destination (`resonance` is an alias of `res`) |
| `env3amt <signed>` | Assignable envelope amount. Octaves for pitch and cutoff. For resonance the useful range is about ±1 |
| `env3attack <ms>` | Assignable envelope attack time |
| `env3decay <ms>` | Assignable envelope decay time |
| `env3sustain <0..1>` | Assignable envelope sustain level |
| `env3release <ms>` | Assignable envelope release time |
| `envcopy` | Copy amp times onto the filter and assignable envelopes |
| `envlink on` / `envlink off` | When on, amp time commands also write the other two envelopes |
| `envvel <0..1>` | Shared velocity scaling for extra envelope amounts |
| `random` | Fill subtractive params plus volume (0.2–1.0); print `eng N` lines including `vol` |

Example (one command per line, or after `/` in keyboard mode):

```text
eng 2 on
eng 2 cutoff 1200
res 0.4
attack 5
release 400
wave square
show
random
```

In **keyboard mode**, press `/` to enter one command line, type the command, then Enter. In **MIDI mode** (line input already active), type the command on its own line, or `q` to quit.

Note release depends on the terminal. Most Unix terminals, including Windows Terminal running WSL, never report that a key came back up, so notes rely on the amp release stage and 4-voice stealing rather than true key-up. The native Windows console (used by `host-windows`) does report key release, so hold-to-play works there.

## Running `engine`'s tests

```bash
cargo test -p engine
```
