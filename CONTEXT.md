# stone-raft

Personal experiment: a portable hardware synthesizer built on a Daisy Seed 3, played as a companion to the author's Polyend Play. It receives MIDI and produces several different sounds at once, one per MIDI channel. Built in Rust as a learn-as-you-go first Rust project.

The author knows Python and data pipelines well, and does not yet know Rust or similar systems languages. There is no traditional software-engineering background. Agents should explain trade-offs in plain language and teach while deciding.

## Language

**Host**:
The thin wrapper program that connects the engine to a specific environment's audio and MIDI. On the laptop there are two hosts (`host-wsl` under WSL, `host-windows` native on Windows) sharing common plumbing; on the Daisy it uses the embedded audio and MIDI drivers.
_Avoid_: runner, container

**Engine**:
One self-contained sound recipe (for example a subtractive synth) assigned to a MIDI channel. Several engines run at once for multitimbral sound.

**Mixer**:
Sums enabled engine instances into one mono output. Disabled instances are skipped so they do not run DSP. Each instance has a volume.
_Avoid_: rack

**Current engine**:
The instance that unqualified terminal commands and keyboard-fallback notes address. MIDI notes ignore this and use each instance’s listen channel.

**Listen channel**:
The MIDI channel (1 through 16) an enabled engine instance responds to. Two instances may share a channel.

**Enabled**:
Whether an engine instance is in the mix loop and accepts notes. Off means no DSP and no sound from that instance.

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

**Filter envelope**:
Dedicated ADSR that moves cutoff over each note. Amount is signed octaves. The cutoff knob is the frequency when this envelope’s level is 0.

**Assignable envelope**:
Third ADSR per voice. Destination is off, resonance, pitch, or cutoff. Terminal commands use the `env3` prefix. More destinations can be added later.
_Avoid_: env2, mod envelope

**Envelope amount**:
How far an envelope moves its destination. Filter amount is octaves. Assignable amount is octaves for pitch and cutoff, and linear for resonance.

**Envelope link**:
When on, amp ADSR commands also write filter and assignable envelope times. A per-envelope time command turns link off.

**Cutoff**:
The filter frequency above which a lowpass turns brightness down.

**Resonance**:
How much the filter boosts frequencies near the cutoff.

**Oscillator mix**:
Per-engine blend of saw, square, triangle, and sine at the note pitch. Each has a level 0 through 1, shared by all voices in that engine. The four at-pitch levels normalize as weights (turning one up does not pull others down in absolute terms, but the engine scales so their sum stays near full scale). Sub mixes on top separately. Level 0 skips that oscillator’s DSP.
_Avoid_: wave switch (solo mode uses the `wave` preset command instead)

**Pulse width**:
How wide the high part of a square cycle is (`0.5` is a classic square; thinner values sound sharper). Only affects the square oscillator in the mix. Clamped away from 0 and 1 so the wave does not go silent.

**Sub oscillator**:
Sine per voice, one or two octaves below the sounding pitch, mixed additively after the normalized at-pitch blend and before the filter. Not part of the four-way normalization. Volume 0 is silent.

**Osc level**:
Mix level for one oscillator in the mix (`sawvol`, `squarevol`, `trianglevol`, `sinevol`, or `subvol`). 0 through 1. Separate from instance `vol`.

**Sub volume**:
Osc level for the sub sine (`subvol`). Additive on top of the normalized at-pitch mix.

**Sub octave**:
1 or 2 octaves below the sounding pitch.

**Wavetable**:
A sound recipe that sweeps through stored waveforms for evolving tones. Planned, not the first engine.

## Decisions

**Rust as the implementation language**
The project is intentionally a first Rust codebase. The goal is to learn the language by building something real (a hardware synth), not to ship the fastest prototype in a familiar stack. Python stays a useful analogy for agents when explaining concepts, not a candidate runtime for the synth engine.

**Daisy Seed as the hardware target**
The instrument runs on a Daisy Seed 3 (Seed3). Same STM32H750 MCU as earlier Seeds, and pin-to-pin compatible with the original Seed pinout and footprint. The audio codec is the TI TAC5242: hardware-strapped, no I2C, up to 32-bit / 192 kHz. Firmware must use the Seed 3 SAI setup (`host-daisy` via daisy-embassy with the `seed3` feature). The engine still starts at 48 kHz mono as decided below. Chosen for a portable, instant-on instrument with onboard audio, powered from a USB powerbank over USB-C. Raspberry Pi remains a known fallback if embedded Rust proves too hard; the portable engine keeps that switch cheap.

Board docs: [Seed3](https://docs.daisy.audio/hardware/Seed3/), [Daisy documentation](https://docs.daisy.audio/), [Seed pinout CSV](https://github.com/electro-smith/DaisyWiki/blob/master/resources/Daisy_Seed_Pinout.csv). [libdaisy-rust](https://github.com/mtthw-meyer/libdaisy-rust) is a reference HAL only; it is not the Daisy host stack.

**Laptop-first, layered architecture**
A Cargo workspace with a shared `engine` crate, shared laptop host plumbing in `host-common`, and thin binaries `host-wsl` and `host-windows` (cpal/midir). Port to the Daisy last (`host-daisy`, daisy-embassy). The engine is the reusable brain; hosts are swappable plumbing. Reevaluate keeping both laptop hosts once the Daisy is in hand.

**Hand-written no_std engine**
The engine is written in no_std style (fixed-size data, no heap, minimal dependencies) from day one, so the exact same DSP runs on the laptop and the Daisy. FunDSP may be studied as a reference but is not a dependency.

**Subtractive engine path and roadmap**
The first engine is subtractive: per-engine oscillator mix (PolyBLEP saw and square with pulse width, PolyBLAMP triangle, pure sine, plus additive sub) into a per-voice state-variable filter (SVF), then a full amp ADSR with exponential-ish segments, then summed. Velocity scales amp with a curved mapping (unit-tested; keyboard still uses fixed velocity). Each voice has three ADSRs: amp, a dedicated filter envelope, and an assignable envelope. Key tracking is later. Planned later on the same recipe: a learning look at Moog-style ladder filters, and a possible reevaluation of heavier band-limited oscillators. Wavetable remains a separate future engine.

**Per-engine oscillator mix**
Five sources per engine: saw, square, triangle, and sine at pitch (levels normalize as weights), plus sub (additive via `subvol`, not in that normalization). Levels are per engine instance, shared by all voices. Default: `sawvol` 1.0, other at-pitch levels 0, `subvol` 0. Level 0 skips that oscillator’s DSP. `wave saw|square|triangle|sine` is a preset shortcut: chosen at-pitch level 1.0, other three 0, `subvol` 0. Pulse width still square-only.

**Dedicated sine sub oscillator**
Each voice has a sine sub mixed after the normalized at-pitch blend and before the filter. Sub pitch tracks the sounding frequency (including env3 pitch), one or two octaves down. `subvol` is 0..1 (default 0, additive); when 0 the sine sample math is skipped. `suboct` is 1 or 2 (default 1).

**Filter and assignable envelopes**
Three ADSRs per voice. Amp owns voice lifetime. Cutoff uses exponential signed-octave modulation; stacking adds octave offsets. Assignable dests are off, resonance, pitch, and cutoff. Times are independent. `envcopy` snapshots amp times onto the other two. `envlink` snaps then follows amp time commands; a filtenv* or env3* time command unlinks. Shared `envvel` defaults to 0; split fvel/e3vel later if needed. Key tracking is not in this slice.

**Terminal param control for laptop development**
Laptop hosts (via `host-common`) change engine params with named line commands. Commands target a current engine (`eng 1` through `eng 4`, 1-based, space required). Unqualified commands hit current. `eng 2 cutoff 800` is one-shot and does not change current. Routing commands: `on`, `off`, `ch <1..16>`, `vol <0..1>`. Oscillator mix: `sawvol`/`sawv`, `squarevol`/`sqvol`, `trianglevol`/`trivol`, `sinevol`/`sinvol`, and `subvol` (each `<0..1>`). `wave saw|square|triangle|sine` is a solo preset (one at-pitch level 1.0, others 0, `subvol` 0). `pulse <0.05..0.95>` for square duty. `suboct 1|2`. `show` prints a replayable qualified patch from a host-side copy (all five osc levels, suboct, and other subtractive params). `random` fills subtractive params including random levels for all five oscs plus volume (0.2–1.0) and does not change on/off or listen channel. Printed patches use `eng N ...` lines with full osc level names. Same commands on `host-wsl` and `host-windows`. Real MIDI CC from the Polyend or other devices are a later session. High-rate knobs/CC may later use atomics plus smoothing; discrete commands and note events use the SPSC queue now.

**Multitimbral routing with per-engine volume**
Four engine instances live in a mixer in the `engine` crate. Instance 1 starts enabled on listen channel 1. Instances 2–4 start disabled, with listen channels 2, 3, and 4 pre-set. MIDI notes fan out to every enabled instance whose listen channel matches. Disabled instances are skipped in the audio loop and ignore notes. `off` silences that instance immediately. Volume is per instance via terminal `vol` (default 1.0). MIDI CC volume waits with the rest of CC mapping. Physical knobs later.

**Per-engine fixed polyphony**
Each engine has a fixed set of 4 voices (tunable after measuring the Daisy). Note-off starts amp release; a voice frees when the amp envelope finishes. When stealing, prefer voices already in release (oldest among those), else the oldest voice overall. Note number → Hz lives in the engine. Voices use a fixed low per-voice gain, velocity curve, and are summed (no divide-by-voice-count). A shared voice pool may come later if channels starve each other.

**Laptop MIDI and keyboard input**
Shared host plumbing opens a midir input when available: auto-select if there is exactly one port, otherwise list ports and pick by number. Note on/off carry MIDI channel and the mixer routes by listen channel. If no MIDI port exists, a crossterm one-octave laptop-keyboard fallback (from C4, fixed velocity) plays the current engine when that engine is on. Param line commands work in both paths. Under WSL, terminal key release is not available, so keyboard-fallback notes rely on amp release and voice stealing. On native Windows (`host-windows`), the console reports key-up so hold-to-play works.

**Host-side patch copy for show**
The audio thread owns the mixer. The host stores params, volume, enabled, and listen channel per instance and updates them when it enqueues instance commands (`MixerEvent::ToInstance`, 1-based). `show` prints that copy. Param apply rules live in the engine crate so `envlink` and `envcopy` match the sounding engine.

**WSL for development, Windows for reliable play**
Edit code and run engine tests in WSL. Optional `host-wsl` is fine for quick checks but WSLg audio is flaky. Reliable listening, real MIDI devices, and keyboard hold-to-play use `host-windows` built with the MSVC toolchain from PowerShell (repo reachable via `\\wsl$\...`). Reevaluate this two-host laptop split when the Daisy arrives.

**Serial MIDI in (Daisy)**
MIDI reaches the Daisy over serial (DIN/TRS) from the Polyend via a MIDI thru box, through an optocoupler into a UART pin. USB MIDI is not used on the Daisy; it is fine on the laptop (via midir) for development against a software keyboard or the Polyend. Note on/off bytes are parsed by `MixerEvent::from_midi_bytes` in the engine crate so laptop and Daisy hosts share one mapping.

**Mono, 48 kHz to start**
The engine produces mono audio at 48 kHz (matching the Daisy codec). The mixer is designed so stereo output and per-engine panning can be added later.

**Flash and debug with a probe**
Flash via the Daisy's USB DFU bootloader, and use a debug probe (ST-Link or similar) from the start for defmt logs and step debugging.

**Lock-free control signals into the audio callback**
The audio callback must never block or wait, since a stall causes audible clicks. Never use a `Mutex` on that path. Note on/off and discrete param changes use a host-owned lock-free SPSC queue (`rtrb` on the laptop). Only the audio thread calls into the engine. On the laptop hosts, a `Mutex` may guard the queue *producer* when both MIDI and the terminal push events; that lock is never taken inside the audio callback. Atomics plus smoothing are reserved for a later high-rate knob/CC path.

**Laptop audio and MIDI device selection**
If there is exactly one output device or one MIDI input port, the host uses it automatically. If there are several, it lists them and asks for a number. Same behavior on `host-wsl` and `host-windows`.
