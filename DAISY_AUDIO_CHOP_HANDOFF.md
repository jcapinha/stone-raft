# Handoff: Daisy audio chopping (breadboard-led)

Read this first, then `CONTEXT.md` and `REJECTED.md`. Treat those two as project truth. Do not re-propose items in `REJECTED.md`.

This is a personal first-Rust synth on a Daisy Seed 3. The author is a data engineer, not a systems/audio person. Explain trade-offs in plain language. Informal tone. Do not commit or open PRs unless asked.

The author will flash the **current tree** after this document was written, then tell you how it sounded. That result is the next fact. Do not assume the latest firmware is already proven on hardware.

## What the user hears

On the Seed, notes **cut up / chop / drop out**, even with a very simple C4–E4–G4 arpeggio (1 s gate, 2 s rest, default saw, engine 1 only, one voice). The breadboard LED still follows the gate correctly. Hello on the OLED still works.

On the laptop hosts (`host-windows` / `host-wsl`) the **same `engine` crate** sounds clean. So this is not “the patch recipe is wrong.” It is the Seed’s realtime path: SAI/DMA/executor, CPU budget, or both.

Quiet line-level into headphones is expected. Chopping is not quiet. It is dropouts.

## Hardware and firmware under test

- Board: Daisy Seed 3 (STM32H750 @ 480 MHz, TI TAC5242 codec, 48 kHz, firmware copies mono to both codec channels).
- Binary: `host-daisy` / `breadboard-led` (`host-daisy/src/bin/breadboard_led.rs`).
- OLED: 0.96" SSD1306 I2C on D11/D12. **Must not talk to the screen while notes sound.** Product UI later is another MCU or a Daisy path that does not pause audio.
- Button D15, gate LED D24.
- Audio: TRRS breakout, Audio Out 1/2 (pins 18/19), AGND pin 20. See `host-daisy/TRRS_BREAKOUT_PROMPT.md` and `host-daisy/BREADBOARD_SETUP_PROMPT.md`.
- No debug probe yet. No defmt logs from the bench unless the author has RTT attached (usually they do not). Listen is the test.

Flash from repo root after `cargo objcopy` to `breadboard-led.bin`:

PowerShell (`\\wsl$\...` repo: set `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL=0` as in README):

```powershell
cargo build -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release -- -O binary breadboard-led.bin
```

WSL: same two cargo lines. Put the Seed in DFU (BOOT+RESET), then `dfu-util -a 0 -s 0x08000000:leave -D breadboard-led.bin`. WSL needs `usbipd` to attach the DFU device.

Always `--release`. Debug builds will lose this race.

## Architecture (do not throw away)

Workspace: `engine` (no_std, shared) + `host-common` + `host-wsl` + `host-windows` + `host-daisy`.

Audio callback must never block. Notes/params go in a lock-free SPSC queue (`heapless` spsc on Daisy). Only the audio side calls `Mixer::next_sample`. Disabled mixer instances skip DSP. Idle voices return 0 without oscillators.

Default first press: engine 1 on, listen ch 1, saw 1.0, other oscs 0, sub 0, filter env amount 0, LFOs dest off. That is the cheapest musical test.

## Symptom vs OLED (important)

The author first blamed the rolling OLED scope. That was **partly true and not the whole story.**

While notes sounded, firmware did a full 128x64 SSD1306 flush on blocking I2C at 400 kHz (~25 ms of bus). Every flush **disabled SPI4**, because audio used to live on an interrupt executor tied to SPI4. Unmasking SPI4 during blocking I2C **corrupts the screen** (`REJECTED.md`). So the audio task froze for the whole flush. FPS dropped to ~0–2 while notes moved. That matches chopped audio.

Grill decision (kept):

- No live scope on the Seed while notes sound.
- No condensed patch card on this blocking I2C path.
- Hello, then sleep. After that, Daisy does not talk to the OLED.
- Later menus/patch card: another microcontroller, or Daisy with DMA/async I2C or SPI, **only if audio is not paused**. Do not pick that hardware now.

**Removing the scope did not stop the chops.** Hello + LED still worked. Sound still broke up. So OLED is closed as the *current* cause. Something else on the Seed still underruns or glitches SAI.

## Timeline of approaches (what we actually did)

### 1. Live rolling scope on I2C (old breadboard-led)

Custom TX-only SAI, audio on SPI4 `InterruptExecutor` at P1, 16384-word TX ring in `.sram1_bss` (RAM_D2), UI drew a scope from an atomic sample ring.

**Result:** audible chops, scope ~1 FPS while notes moved. Documented as “acceptable for bring-up.” It was not.

### 2. Drop scope and patch card, keep Hello, shrink DMA to crate default 128, start codec only after Hello sleep

Goal: never I2C after audio, snappy notes.

**Result:** Hello and gate LED worked. **No sound at all.** 128-word ring + SPI4 interrupt executor + delayed SAI start is a known silent combo in this project. Closed in `REJECTED.md` as “shrinking DMA to 128 while audio ran on SPI4.”

### 3. Restore 16384 ring, start audio as soon as Hello is drawn (like the last-known-working main), still no scope

**Result:** sound came back. **Still chopped like crazy.** So the giant ring was a crutch for “some audio exists,” not for clean audio. Enlarging DMA to hide stalls is already rejected as a strategy.

### 4. Keep 16384 and early audio start, move `audio_loop` onto the thread-mode `Spawner` (drop SPI4 interrupt executor)

Theory: the audio task was filling a huge ring inside a high-priority interrupt, so DMA completion could not keep up, embassy ring overran, firmware did `warn!` + `Timer::after_millis(1)` and you heard holes.

**Result (author listened):** still chopped. Thread executor alone did not fix it.

### 5. Current tree (NOT heard yet when this file was written)

Do this next listen test.

Firmware (`breadboard_led.rs`):

- Hello 3 s, sleep, **then** `prepare_interface` + `start_interface` + `start_callback` (daisy-embassy Seed 3 path, TX **and** RX, crate DMA size 128 words / 32 stereo frames per half).
- Mixer in a `StaticCell` (too big for the embassy task stack).
- Callback: dequeue SPSC events, `mixer.next_sample()` into both channels, `f32_to_u24`.
- On SAI error, warn and restart `start_callback` (no 1 ms sleep).
- Button/LED on the thread executor beside that task.
- No custom `AUDIO_TX_BUFFER`, no `grounded` ring, no SPI4 audio interrupt.

Engine (aimed at CPU, same crate the laptop uses):

- SVF (`engine/src/filter.rs`): Cytomic / Andy Simper linear trapezoidal lowpass. **`tanf`/`expf` only when cutoff or resonance change.** Default patch has filter-env amount 0, so first-press C-E-G should hit the cache every sample after the first.
- `hz_times_octaves`: skip `powf` when octaves is 0.
- LFOs: skip `next_level` when dest is off or amount is 0.

`cargo test -p engine`: 79 passed after those engine edits.

**Why we switched to `start_callback`:** the custom TX-only SAI existed because RX overran while the OLED blocked the CPU (older session). OLED no longer runs during notes. The homemade TX loop + 16384 ring + overrun sleep is a Daisy-only footgun the laptop never hits. Seed 3 examples in daisy-embassy use read → callback → write.

**Risk:** Hello-then-start-codec previously meant silence, but that was with SPI4 + 128 custom TX. This path is different. If the author reports silence again, compare to that old combo before assuming the codec “must start at boot.”

## How to read the author’s next message

- **First-press C-E-G is clean:** the callback + cheaper default-patch DSP is enough for 1 saw voice. Next: `random` (all oscs, filter env, LFOs). If random chops and default does not, it is CPU. Profile / cheapen modulated SVF, osc mix, libm.
- **Still chopped on first-press saw:** not “filter env is too heavy.” Look at SAI/DMA/callback timing, blocking on the thread executor, cache/memory, or still overrunning 32-frame blocks.
- **Silence, LED still gates:** SAI never started or `start_callback` dies immediately. Check spawn order (audio after Hello sleep), `start_interface`, panic. Last silent bug was 128 + SPI4 interrupt executor, which this tree no longer uses.
- **Clicks only at Hello sleep:** leftover I2C vs audio overlap. Current code sleeps the OLED *before* `audio_loop` spawn.

## Likely remaining causes (if chops persist)

1. **Callback too slow for 32 frames / 0.67 ms** even for 1 saw + SVF. Laptop buffers are huge. Daisy is not. Confirm with a **sine-only or silence callback** that does not call `Mixer`. If a raw triangle like `daisy-embassy` `examples/triangle_wave_tx.rs` is clean on this same board/jack, the engine is over budget. If the triangle also chops, the host SAI path or hardware is wrong.

2. **`libm` transcendentals on M7.** `tanf`/`expf`/`sinf`/`powf` are in the voice path when modulation is on (`engine/src/filter.rs`, `oscillator.rs`, `lfo.rs`, `hz_times_octaves`). Caching helps the default patch only.

3. **Per-sample coeff updates** when filter env / LFO / assignable env move cutoff. Analog-style tracking wants that. Embedded practice is often **block-rate** coeff updates (once per 32 samples). DaisySP and many PD objects do not recompute SVF `g` every sample unless told to.

4. **Thread executor shared with `ui_loop`.** `wait_for_press` polls every 10 ms. `play_arpeggio` uses `Timer::after_millis`. Those yield. Do not add blocking I2C, `defmt` in the callback, or long `warn!` on the audio task. RTT can stall if a probe is attached.

5. **RX+TX `start_callback` must keep both directions fed.** If the callback is late, embassy returns SAI error and we restart. That restart is itself a glitch. Measure how often `audio callback stopped; restart` would fire (needs RTT/probe).

6. **STM32H7 D-cache vs DMA.** daisy-embassy puts TX/RX in `.sram1_bss` → RAM_D2 (`0x30000000`). That is the vendor pattern. Do not move the DMA buffer to DTCM/AXI SRAM without cache maintenance.

7. **Four voices always allocated, but idle voices skip DSP.** Four engines: 2–4 start disabled and are skipped in `Mixer::next_sample`. Do not “fix” chops by shrinking polyphony until a sine callback is proven.

## Engine DSP as it stands (for the next person who optimizes)

Per active voice, each sample (`engine/src/voices.rs`):

- Amp, filter, and assignable ADSRs.
- Optional two LFOs.
- Pitch/cutoff via `hz_times_octaves` (pow2).
- Osc mix: saw PolyBLEP, square PolyBLEP + pulse width, triangle PolyBLAMP, sine `libm::sinf`, additive sine sub. Level 0 skips that osc.
- SVF lowpass every sample.

References (study, do not add FunDSP as a dependency: `REJECTED.md`):

### Official Daisy / C++

- [electro-smith/DaisySP](https://github.com/electro-smith/DaisySP) — `Svf`, `Oscillator`, `PolyBlepOsc`, `Adsr`. This is what the C++ Daisy world actually runs at 48 kHz on the same MCU class.
- [electro-smith/libDaisy](https://github.com/electro-smith/libDaisy) — audio callback size, SAI, DMA. Seed 3 notes in Daisy docs: [Seed3](https://docs.daisy.audio/hardware/Seed3/).
- [electro-smith/DaisyExamples](https://github.com/electro-smith/DaisyExamples) — smallest synth examples for “does the board play a tone.”

### Rust on Daisy

- [daisy-embassy](https://github.com/daisy-embassy/daisy-embassy) 0.3, feature `seed3`. **Copy `examples/triangle_wave_tx.rs` into a throwaway bin** if you need a host-only sine/triangle to split “codec/DMA” from “engine.” TAC5242 L/R swap is handled inside `src/codec/tac5242.rs`.
- [mtthw-meyer/libdaisy-rust](https://github.com/mtthw-meyer/libdaisy-rust) — HAL reference only, not this host stack (`CONTEXT.md`).

### Filters (the SVF we already use)

- Andy Simper / Cytomic: [SvfLinearTrapOptimised2.pdf](https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf). Comment in `engine/src/filter.rs` already names this. Look at how they update `g` (often once per block).
- DaisySP `Source/Filters/svf.cpp`.
- [vinniefalco/DSPFilters](https://github.com/vinniefalco/DSPFilters) — not for no_std copy-paste, good for coeff vs sample split.

### Oscillators / anti-aliasing

- Välimäki & Huovilainen PolyBLEP (papers; many C++ ports). DaisySP `PolyBlepOsc`.
- [pure-data/pure-data](https://github.com/pure-data/pure-data) — `phasor~`, `osc~` (cosine table, not naive `sinf` every sample). Table lookup + interpolation is the usual embedded sine.
- [pure-data/purr-data](https://github.com/agraef/purr-data) or vanilla help for `[vcf~]`, `[lop~]`, `[bp~]`. Cyclone `svf~` if you want an SVF object to compare block behaviour.
- Mutable Instruments: [pichenettes/stmlib](https://github.com/pichenettes/stmlib), [eurorack / plaits](https://github.com/pichenettes/eurorack) — oscillators and SVF on Cortex-M, very budget-aware.

### FunDSP (read only)

- [FunDSP](https://github.com/SamiPerttu/fundsp) — `CONTEXT.md`: learning reference, **not** a Daisy dependency (needs std/heap).

## Suggested next work (in order)

1. Get the author’s listen result for **this tree**.
2. If still bad: add a **temporary** `breadboard-led` path or bin that only outputs a triangle/sine in `start_callback` (port daisy-embassy example). Same jack, same flash process.
3. If triangle is clean: cheapen the engine for M7 (block-rate SVF `g`, sine LUT, do not `powf`/`tanf` per sample when modulated; maybe compute filter coeffs once per callback block of 32). Keep laptop tests green (`cargo test -p engine -p host-common`).
4. If triangle is dirty: stay in `host-daisy` / daisy-embassy / SAI. Do not rewrite oscillators yet.
5. Do not bring back the OLED scope or SPI4-masked I2C during notes.
6. Do not grow the DMA ring again to hide glitches.
7. Serial MIDI is the next bring-up item in `CONTEXT.md` **after** this arpeggio is clean.

## Files that matter

| File | Why |
|------|-----|
| `host-daisy/src/bin/breadboard_led.rs` | Current Daisy host |
| `engine/src/voices.rs` | Per-voice render |
| `engine/src/filter.rs` | SVF + coeff cache |
| `engine/src/oscillator.rs` | PolyBLEP / sine |
| `engine/src/mixer.rs` | Enabled-instance mix |
| `CONTEXT.md` / `REJECTED.md` | Decisions |
| `README.md` | Flash commands |
| daisy-embassy 0.3 `src/codec/tac5242.rs`, `src/audio.rs`, `examples/triangle_wave_tx.rs` | Vendor audio path (in cargo registry) |
