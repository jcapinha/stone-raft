# stone-raft

Personal experiment: a portable hardware synthesizer built on a Daisy Seed, played as a companion to the author's Polyend Play. It receives MIDI and produces several different sounds at once, one per MIDI channel. Built in Rust as a learn-as-you-go first Rust project.

The author knows Python and data pipelines well, and does not yet know Rust or similar systems languages. There is no traditional software-engineering background. Agents should explain trade-offs in plain language and teach while deciding.

## Language

**Host**:
The thin wrapper program that connects the engine to a specific environment's audio and MIDI. On the laptop it uses cpal; on the Daisy it uses the embedded audio and MIDI drivers.
_Avoid_: runner, container

**Engine**:
One self-contained sound recipe (for example a subtractive synth) assigned to a MIDI channel. Several engines run at once for multitimbral sound.

**Voice**:
One sounding note or layer the synth is generating at a moment in time.
_Avoid_: channel (unless meaning audio output channel or MIDI channel)

**Multitimbral**:
Producing several different sounds at the same time, each responding to its own MIDI channel.

**Polyphony**:
How many voices an engine can sound at once. Needed to play chords.

**Subtractive**:
A sound recipe: start with a harmonically rich waveform, then shape it with a filter and an amplitude envelope (ADSR).

**Wavetable**:
A sound recipe that sweeps through stored waveforms for evolving tones. Planned, not the first engine.

## Decisions

**Rust as the implementation language**
The project is intentionally a first Rust codebase. The goal is to learn the language by building something real (a hardware synth), not to ship the fastest prototype in a familiar stack. Python stays a useful analogy for agents when explaining concepts, not a candidate runtime for the synth engine.

**Daisy Seed as the hardware target**
The instrument runs on a Daisy Seed (Rev7 / Seed 1.2, PCM3060 codec). Chosen for a portable, instant-on instrument with onboard audio and analog knob inputs, powered from a USB powerbank. Raspberry Pi remains a known fallback if embedded Rust proves too hard; the portable engine keeps that switch cheap.

**Laptop-first, layered architecture**
A Cargo workspace with a shared `engine` crate and thin host crates. Build and hear the synth on the laptop first (`host-laptop`, cpal), port to the Daisy last (`host-daisy`, daisy-embassy). The engine is the reusable brain; hosts are swappable plumbing.

**Hand-written no_std engine**
The engine is written in no_std style (fixed-size data, no heap, minimal dependencies) from day one, so the exact same DSP runs on the laptop and the Daisy. FunDSP may be studied as a reference but is not a dependency.

**Subtractive first; wavetable planned**
The first engine is subtractive (oscillator to filter to ADSR amp). Wavetable is an explicitly wanted future engine. Multitimbral routing does not care what recipe each engine uses.

**Multitimbral routing with per-engine volume**
Incoming MIDI is routed by channel to a matching engine; engine outputs are summed into one mix. Each engine's volume (gain) is adjustable live via MIDI CC now, and physical knobs later.

**Per-engine fixed polyphony**
Each engine has its own fixed set of voices (around 4, tunable after measuring the Daisy) with voice stealing when notes exceed voices. A shared voice pool may come later if channels starve each other.

**Serial MIDI in**
MIDI reaches the Daisy over serial (DIN/TRS) from the Polyend via a MIDI thru box, through an optocoupler into a UART pin. USB MIDI is not used on the Daisy; it is fine on the laptop (via midir) for development against a software keyboard or the Polyend.

**Mono, 48 kHz to start**
The engine produces mono audio at 48 kHz (matching the Daisy codec). The mixer is designed so stereo output and per-engine panning can be added later.

**Flash and debug with a probe**
Flash via the Daisy's USB DFU bootloader, and use a debug probe (ST-Link or similar) from the start for defmt logs and step debugging.
