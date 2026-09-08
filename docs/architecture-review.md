#### Architecture Review — 2026-09-07

**Summary**

`breadboard_led.rs` (~597 lines) is large for a single binary, but not because one concept is bloated. It is doing the work of several modules that do not exist yet: Daisy audio adapter, interrupt-safe OLED I/O, rolling scope rendering, condensed patch card formatting, and a breadboard demo state machine. That matches the current bring-up stage (codec + engine + OLED before serial MIDI), yet it conflicts with the project's own layering: laptop hosts share `host-common` while `host-daisy` has no `lib.rs` and only two binaries (`double_blink` at 30 lines, `breadboard_led` at 597). The file is understandable today; the friction is that reusable host plumbing is trapped inside a throwaway demo binary. When the permanent Daisy host arrives, much of this will need to move or be rewritten unless deepened first.

---

**Candidates**

### `host-daisy` crate layout — no shared library, all plumbing in one binary

**Problem**

`host-daisy` exposes only `[[bin]]` targets. Every Daisy-specific adapter (SAI/DMA audio loop, event queue wiring, OLED) lives inline in `breadboard_led.rs`. The deletion test fails badly: delete the binary and all host depth vanishes, even though CONTEXT.md describes a future permanent Daisy host that will need the same audio path and OLED patch card.

**Deepening opportunity**

Add `host-daisy/src/lib.rs` as the Daisy host library (mirroring `host-common` on the laptop). Binaries become thin: init hardware, spawn tasks, run demo-specific UI. Hidden inside the library: audio executor setup, mixer + SPSC queue integration, OLED helpers, optional scope and patch card formatters.

**Before / After sketch**

```python
# Before — everything in breadboard_led.rs main binary
def main():
    setup_sai_dma_interrupt_loop(mixer, queue)
    setup_oled_with_spi4_masking()
    ui_demo_loop(button, led, scope, patch_card)

# After — lib owns adapters, binary owns demo
# host-daisy/src/lib.rs
def run_audio(mixer_events_consumer, scope_sink) -> AudioHandle: ...
def oled_128x64(i2c_pins) -> OledDisplay: ...

# host-daisy/src/bin/breadboard_led.rs (~150 lines)
def main():
    audio = host_daisy.run_audio(consumer, SCOPE)
    display = host_daisy.oled_128x64(d11, d12)
    breadboard_demo_loop(audio, display, button, led)
```

**Recommendation strength**: Strong

**ADR conflict** (if any): None. Aligns with CONTEXT.md "thin wrapper program" and "layered architecture."

---

### Daisy audio adapter — SAI setup + interrupt executor loop (~100 lines)

**Problem**

`prepare_audio_output`, `audio_loop`, static DMA buffer, and `f32_to_u24` are the real host seam (engine → codec at 48 kHz). They sit inside a breadboard demo binary. Shallow as a cluster: the interface to callers is essentially "spawn this task with these peripherals," but the implementation is substantial and will be needed again for MIDI bring-up.

**Deepening opportunity**

One deep module in `host-daisy`: `run_mixer_audio(peripherals, consumer, on_sample?)` that owns SAI config, DMA buffer, overrun recovery, and the mixer sample loop. Optional callback or atomic sink for scope samples keeps display logic out of the audio path (matches CONTEXT.md: display drawing stays off the audio callback path).

**Before / After sketch**

```python
# Before
async def audio_loop(peripherals, consumer):
    mixer = MIXER.init(...)
    output = prepare_audio_output(peripherals)  # 40 lines SAI config
    loop: dequeue events, mixer.next_sample(), scope push, DMA write

# After
audio = DaisyAudio::start(peripherals, consumer, ScopeTap::new(&SCOPE))
# SAI config, DMA, overrun handling, sample loop — all hidden
```

**Recommendation strength**: Strong

**ADR conflict** (if any): None.

---

### OLED I2C with SPI4 interrupt masking — `with_oled_bus` cluster (~30 lines + call sites)

**Problem**

Blocking I2C on the main executor must not race the audio interrupt executor. The masking pattern (`with_oled_bus`) is correct but scattered across `init_oled`, `oled_flush`, `oled_set_on`. Callers must remember to use these wrappers. Two adapters are not yet present (only this OLED), but the behaviour is non-obvious and will recur on any Daisy firmware with OLED + audio.

**Deepening opportunity**

Wrap the display in a small module whose **interface** is "flush / set_on / draw" and whose **implementation** always handles SPI4 masking internally. Callers never touch `with_oled_bus` directly. Depth comes from hiding the interrupt choreography.

**Before / After sketch**

```python
# Before
def oled_flush(display):
    with_oled_bus(lambda: display.flush())

# After
display = MaskedOled::new(i2c, audio_irq=SPI4)
display.flush()  # masking always inside
```

**Recommendation strength**: Worth exploring (becomes Strong when the permanent Daisy host lands)

**ADR conflict** (if any): None.

---

### Rolling scope — `ScopeBuffer` + `draw_scope` (~90 lines)

**Problem**

Generic behaviour (atomic ring buffer fed from audio, downsampled snapshot, embedded-graphics line plot with auto-scaling) lives in the demo binary. It is deeper than a pass-through and could serve the final instrument if the OLED keeps a rolling scope. Right now it is coupled to SSD1306 types and breadboard constants.

**Deepening opportunity**

Extract a `RollingScope` module: push sample from audio side, `render_to(display, frame_ms)` on UI side. Display adapter takes anything implementing a minimal draw trait (or a closure for line drawing). Constants (`SCOPE_LEN`, downsample) become module config.

**Before / After sketch**

```python
# Before
SCOPE.push(sample)  # in audio_loop
draw_scope(display, new_count)  # knows SSD1306, audio_state, scaling

# After
scope = RollingScope(len=128, downsample=8)
scope.push(sample)
scope.draw_waveform(display, baseline_y=32)  # scaling hidden
```

**Recommendation strength**: Worth exploring (Strong if rolling scope stays on the shipping Daisy UI; Speculative if OLED moves to patch-card-only)

**ADR conflict** (if any): None. CONTEXT.md lists "rolling scope" as a product feature.

---

### Condensed patch card — `patch_card_lines` + `PatchShadow` (~50 lines)

**Problem**

Eight-line OLED summary duplicates knowledge already split between `engine` (`EngineParams`, `patch_events`, `random_patch`) and `host-common` (`format_show` for laptop). The formatter is shallow: interface is nearly as large as the implementation (manual `write!` per field, local `dest_name` match). Two hosts now format the same patch differently with no shared module.

**Deepening opportunity**

Move formatting into `engine` (no_std, no I/O) as `condensed_patch_lines(params, volume) -> [FixedString; 8]`, parallel to how `random_patch` and `patch_events` already live in the engine for shared laptop/Daisy use. Daisy host only draws strings; laptop could reuse for a future compact mode.

**Before / After sketch**

```python
# Before — in breadboard_led.rs
def patch_card_lines(shadow):
    write 8 lines from shadow.params and shadow.volume

# After — in engine
lines = engine.condensed_patch_lines(params, volume)
for i, line in enumerate(lines):
    oled.draw_text(0, i * 8, line)
```

**Recommendation strength**: Worth exploring

**ADR conflict** (if any): None. Matches CONTEXT.md "Host-side patch copy" and "Condensed patch card."

---

### Breadboard demo logic — `ui_loop`, `play_arpeggio`, button debounce (~100 lines)

**Problem**

This is the only part that is genuinely demo-specific (C-E-G arpeggio, first-press vs random, boot flash, debounce). It is appropriately specific to this binary.

**Deepening opportunity**

Keep it in `breadboard_led.rs`. Do not extract unless a second demo binary needs the same arpeggio pattern (unlikely). If `host-daisy` lib exists, this file should shrink to ~100–150 lines of demo orchestration.

**Before / After sketch**

```python
# This stays in the binary — correct place
async def ui_loop(...):
    wait_for_press()
    if not first: apply_random()
    play_arpeggio(CEG, gate=1s, rest=2s)
    show_patch_card()
```

**Recommendation strength**: Speculative (no deepening needed; deletion test says keep in binary)

**ADR conflict** (if any): None.

---

## Top recommendation

**Do not split `breadboard_led.rs` into many small files right now.** The size is a symptom of missing `host-daisy` library depth, not of one overgrown function. Tack **`host-daisy` crate layout + Daisy audio adapter** first when you start serial MIDI bring-up (the next CONTEXT.md milestone). That is when the demo binary stops being the only Daisy host and duplication cost becomes real.

For today, the 597-line file is **normal for a monolithic embedded bring-up binary**, but **not normal for this repo's architecture** once you compare it to `host-common` + thin laptop binaries. Optional low-cost improvement without new files: add `mod audio; mod display; mod scope; mod demo;` under `host-daisy/src/` and move sections out of the binary — only if navigation pain is high; the skill prefers fewer broader modules over fragmentation.

**Direct answer:** the file is huge because it is five modules wearing one filename, not because breadboard demos must be 600 lines. Extraction is worth doing when the permanent Daisy host starts, not urgently for a working bring-up test.
