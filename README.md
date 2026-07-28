# stone-raft

A portable hardware synthesizer built on a Daisy Seed, written in Rust as a first Rust project. See [`CONTEXT.md`](CONTEXT.md) for the full project description.

## Project context

Living project truth for humans and agents:

- [`CONTEXT.md`](CONTEXT.md) — current project intent, domain language, and decisions
- [`REJECTED.md`](REJECTED.md) — closed doors (do not re-suggest these)

To stress-test a plan and update those files as you decide, use `/grill-with-docs` in this repo (project skill only; separate from personal `/grill-me`).

## Workspace layout

- [`engine/`](engine) — the sound-generation code (oscillators, filters, envelopes). Written `no_std` so the same code can run on the Daisy Seed later, with no operating system underneath.
- [`host-laptop/`](host-laptop) — a thin adapter that connects `engine` to your laptop's speakers via [cpal](https://docs.rs/cpal). Used for fast development before porting to the Daisy.

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

No extra system packages are needed; `cpal` uses WASAPI (Windows' native audio API) automatically.

```powershell
cargo run -p host-laptop
```

Once running, press Enter to toggle a 440 Hz test tone on/off. Type `q` then Enter to quit.

## Running `engine`'s tests

```bash
cargo test -p engine
```
