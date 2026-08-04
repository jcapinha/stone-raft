# stone-raft

Personal experiment: a portable hardware synthesizer built on a Daisy Seed, played as a companion to the author's Polyend Play. It receives MIDI and produces several different sounds at once, one per MIDI channel. Built in Rust as a learn-as-you-go first Rust project.

The author knows Python and data pipelines well, and does not yet know Rust or similar systems languages. There is no traditional software-engineering background. Agents should explain trade-offs in plain language and teach while deciding.

## Language

**Host**:
The thin wrapper program that connects the engine to a specific environment's audio and MIDI. On the laptop there are two hosts (`host-wsl` under WSL, `host-windows` native on Windows) sharing common plumbing; on the Daisy it uses the embedded audio and MIDI drivers.
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

**Envelope / ADSR**:
A timed shape that runs when a note starts and ends. Attack, Decay, Sustain, and Release describe how a level rises, falls to a held value, then fades after note-off.
_Avoid_: using “envelope” alone when amp vs filter vs assignable must be distinguished

**Cutoff**:
The filter frequency above which a lowpass turns brightness down.

**Resonance**:
How much the filter boosts frequencies near the cutoff.

**Wavetable**:
A sound recipe that sweeps through stored waveforms for evolving tones. Planned, not the first engine.

## Decisions

**Rust as the implementation language**
The project is intentionally a first Rust codebase. The goal is to learn the language by building something real (a hardware synth), not to ship the fastest prototype in a familiar stack. Python stays a useful analogy for agents when explaining concepts, not a candidate runtime for the synth engine.

**Daisy Seed as the hardware target**
The instrument runs on a Daisy Seed (Rev7 / Seed 1.2, PCM3060 codec). Chosen for a portable, instant-on instrument with onboard audio and analog knob inputs, powered from a USB powerbank. Raspberry Pi remains a known fallback if embedded Rust proves too hard; the portable engine keeps that switch cheap.

**Laptop-first, layered architecture**
A Cargo workspace with a shared `engine` crate, shared laptop host plumbing in `host-common`, and thin binaries `host-wsl` and `host-windows` (cpal/midir). Port to the Daisy last (`host-daisy`, daisy-embassy). The engine is the reusable brain; hosts are swappable plumbing. Reevaluate keeping both laptop hosts once the Daisy is in hand.

**Hand-written no_std engine**
The engine is written in no_std style (fixed-size data, no heap, minimal dependencies) from day one, so the exact same DSP runs on the laptop and the Daisy. FunDSP may be studied as a reference but is not a dependency.

**Subtractive engine path and roadmap**
The first engine is subtractive: one oscillator per voice (PolyBLEP saw or square, live waveform select) into a per-voice state-variable filter (SVF) into a full amp ADSR with exponential-ish segments, then summed. Velocity scales amp with a curved mapping (unit-tested; keyboard still uses fixed velocity). End state for this recipe is three envelopes: amp (now), filter (next session after this), then a third assignable to other parameters. Planned later on the same recipe: pulse width, dual-oscillator mix, a learning look at Moog-style ladder filters, and a possible reevaluation of heavier band-limited oscillators. Wavetable remains a separate future engine.

**Terminal param control for laptop development**
While developing without a MIDI device, the laptop hosts (via `host-common`) change engine params via named line commands (cutoff in Hz, resonance and sustain in 0–1, ADSR times in milliseconds, wave select). Real MIDI CC from the Polyend or other devices are a later session. High-rate knobs/CC may later use atomics plus smoothing; discrete commands and note events use the SPSC queue now.

**Multitimbral routing with per-engine volume**
Incoming MIDI is routed by channel to a matching engine instance; engine outputs are summed into one mix. Each instance's volume (gain) is adjustable live via MIDI CC now, and physical knobs later. Longer term: configurable channel → instance mapping, including several instances of the same recipe with independent params (filter, ADSR, etc.). Not built yet.

**Per-engine fixed polyphony**
Each engine has a fixed set of 4 voices (tunable after measuring the Daisy). Note-off starts amp release; a voice frees when the envelope finishes. When stealing, prefer voices already in release (oldest among those), else the oldest voice overall. Note number → Hz lives in the engine. Voices use a fixed low per-voice gain, velocity curve, and are summed (no divide-by-voice-count). A shared voice pool may come later if channels starve each other.

**Laptop MIDI and keyboard input**
Shared host plumbing opens a midir input when available: auto-select if there is exactly one port, otherwise list ports and pick by number. Note on/off feed the engine. If no MIDI port exists, a crossterm one-octave laptop-keyboard fallback (from C4, fixed velocity) pushes the same events. Param line commands work in both paths. This development slice listens on all MIDI channels with a single engine instance. Under WSL, terminal key release is not available, so keyboard-fallback notes rely on amp release and voice stealing. On native Windows (`host-windows`), the console reports key-up so hold-to-play works.

**WSL for development, Windows for reliable play**
Edit code and run engine tests in WSL. Optional `host-wsl` is fine for quick checks but WSLg audio is flaky. Reliable listening, real MIDI devices, and keyboard hold-to-play use `host-windows` built with the MSVC toolchain from PowerShell (repo reachable via `\\wsl$\...`). Reevaluate this two-host laptop split when the Daisy arrives.

**Serial MIDI in (Daisy)**
MIDI reaches the Daisy over serial (DIN/TRS) from the Polyend via a MIDI thru box, through an optocoupler into a UART pin. USB MIDI is not used on the Daisy; it is fine on the laptop (via midir) for development against a software keyboard or the Polyend.

**Mono, 48 kHz to start**
The engine produces mono audio at 48 kHz (matching the Daisy codec). The mixer is designed so stereo output and per-engine panning can be added later.

**Flash and debug with a probe**
Flash via the Daisy's USB DFU bootloader, and use a debug probe (ST-Link or similar) from the start for defmt logs and step debugging.

**Lock-free control signals into the audio callback**
The audio callback must never block or wait, since a stall causes audible clicks. Never use a `Mutex` on that path. Note on/off and discrete param changes use a host-owned lock-free SPSC queue (`rtrb` on the laptop). Only the audio thread calls into the engine. On the laptop hosts, a `Mutex` may guard the queue *producer* when both MIDI and the terminal push events; that lock is never taken inside the audio callback. Atomics plus smoothing are reserved for a later high-rate knob/CC path.

**Laptop audio and MIDI device selection**
If there is exactly one output device or one MIDI input port, the host uses it automatically. If there are several, it lists them and asks for a number. Same behavior on `host-wsl` and `host-windows`.
