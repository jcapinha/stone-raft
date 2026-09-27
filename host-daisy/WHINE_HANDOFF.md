# Callback whine handoff

Status: cause confirmed, fix not chosen. Do not change code, flash, or move wires until the author picks an option in "Open choice" below.

Listens were on a USB powerbank, not the PC, so the PC USB ground was already out of the picture. The breadboard was not changed during the proof.

## What is confirmed

The thin high lines are [callback noise](https://docs.daisy.audio/tutorials/eliminating-callback-noise/): a power kick once per audio batch. The codec runs at 48 kHz. The kick's pitch is `48000 / batch size`, plus quieter copies at whole-number multiples.

| Batch | Kick rate | Lines the author read | Firmware |
|---|---|---|---|
| 32 samples (published daisy-embassy 0.3.0) | 1,500 Hz | about 1,500 Hz, then about 3,000 Hz | 240 Hz sine, engine bypassed |
| 16 samples (the tree right now) | 3,000 Hz | about 3,000 Hz, then about 6,000 Hz | same sine, same wires, same powerbank |

The 16-sample flash changed only the batch size. The lines moved. That is the proof.

Daisy's page leads with a 1 kHz example. That number is the same kick with a batch of 48 samples (`48000 / 48`). This project uses 32, so the pitch is 1,500 Hz, not 1 kHz.

Silence before any click has no audible tone. The callback still runs, but it only writes zeros. A sounding note makes the kick loud enough to hear. The author said the whine's partials change volume across probe clicks. On richer waves that is mostly the note. On click 3 the high lines got shorter and taller with the filter LFO. Those moving lines are the patch. The sine stripped the patch down to one peak at 240 Hz and left the block-rate lines behind.

The hover on the 32-sample sine was approximate ("basically 1,500" and "basically 3,000"). The 16-sample move is the precise check. Do not reopen "maybe it was the 6th harmonic of 240 Hz" unless a new listen puts a line back at 1,500 Hz while the batch is 16.

## What a quieter ground does, and what it does not

It does not remove the kick, and it does not change the pitch. The author is right to doubt a ground fix as a cure.

The program sends a clean beep. The headphones measure voltage between an audio pin and ground. Each batch ends with a short spike in the chip's current. That spike flows through ground. Ground wires are not a perfect zero: a current spike makes them jump by a tiny voltage. The headphones treat that jump as sound. One jump per batch is a tone at the batch rate.

A shorter or separate audio ground can make that jump smaller in the headphones, so the same 1,500 Hz tone gets quieter. It can also do nothing useful. The jump Daisy describes can happen inside the Seed, between the MCU and the codec, on a board the author did not lay out. External jumpers cannot reach that part. Daisy's layout advice is about the PCB you design around the Seed. A breadboard is only the external half.

What is already done: AGND (pin 20) and DGND (pin 40) are tied, which is the tie Daisy wants. The tie is long. The author described it as:

- Pin 20 to one ground rail, together with the TRRS sleeve and RING2.
- A jumper from that rail to the other side of the breadboard.
- On the other rail: OLED GND, button GND, and pin 40.

Headphone ground, digital ground, the screen, and the button share that path. Digital current from the screen and the button can use the same wire the headphones use as zero. Shortening that, or giving the jack its own short wire to pin 20 and meeting pin 40 at one point, is a loudness experiment. Success looks like the 1,500 Hz line getting shorter on the spectrum, not moving. Failure looks like the line staying just as tall. Either result is informative. Neither result changes the batch math.

Documented headphone circuit, not re-checked this session: Audio Out 1 and 2 (pins 18 and 19) through 10 µF caps and 100 Ω resistors into the TRRS tip and ring, mono copied to both codec channels. OLED power is digital 3.3 V (pin 38). Do not use analog 3.3 V on pin 21. See the breadboard section of `README.md`.

## Open choice

The author is deciding. Do not pick for them.

1. Put the published 32-sample batch back, then try a shorter headphone ground. Keeps the voice counts already measured. The whine stays at 1,500 Hz unless the external ground was carrying most of the kick. About 15 minutes of wiring plus one sine listen if the 32-sample sine is flashed again.
2. Use a batch of 2 samples so the kick sits at 24 kHz, which most adults cannot hear. Daisy's software fix. Costs spare CPU on every Daisy flash. The heavy-patch voice counts below were measured at 32 samples and would need a new probe listen, about 20 minutes. A batch of 4 samples lands at 12 kHz, still a whistle. A batch of 16 samples is the confirmation flash, not a fix. 3,000 Hz is still audible.

Daisy also suggests burning extra CPU outside the callback so the current spike is less sharp. They call that less predictable. It does not move the pitch. Do not lead with it.

## What a smaller batch costs

The engine does not get simpler. Oscillators, filter, envelopes, MIDI, and the laptop hosts stay as they are. `host-wsl` and `host-windows` do not use daisy-embassy and never had this whine. The kick is electrical, on the Seed, not a tone in the samples.

`BLOCK_LENGTH` in daisy-embassy is the one number. `audio-probe` and `breadboard-led` both call `prepare_interface(Default::default())` and inherit it. A future Daisy firmware will too. The engine's `render_block` already accepts any slice length, so a shorter batch still produces the same samples. Filter and LFO control steps run once per batch, so they update a bit more often. That is not a capability loss.

The loss is time per visit. At 480 MHz:

| Batch | Time per visit | Cycle budget | Kick |
|---|---|---|---|
| 32 | about 0.67 ms | 320,000 (`CALLBACK_CYCLES` in `audio_probe.rs`) | 1,500 Hz |
| 16 | about 0.33 ms | 160,000 | 3,000 Hz |
| 2 | about 0.04 ms | 20,000 | 24,000 Hz |

The LED blink buckets still divide by 320,000. On the 16-sample flash they are meaningless. Ignore them. A continuous rapid blink still means the audio interface failed.

Voice baseline, all at a 32-sample batch, instruction cache on. Do not overwrite these with 16-sample blinks:

- Data cache off, sine table in DTCM, heavy patch volume 0.7: clicks 1 through 6 were one blink (2026-09-25).
- Data cache on: click 10 was two blinks and audio kept going. Click 11 (13 heavy voices) was three blinks and audio kept going. Click 12 (16 heavy voices) blinked continuously and audio stopped (2026-09-27). Full click map is in `README.md` under "Daisy audio probe" and in the `Daisy audio-probe stress test` decision in `CONTEXT.md`.

`breadboard-led` is one light chord. It would likely still play on a short batch. Its data cache stays off. Building it today, with the patch below still in the tree, would ship 16-sample audio.

## Tree right now

Temporary. Leave it until the author chooses. Restoring early throws away the firmware that just proved the move, which is fine only after they say so.

- `host-daisy/vendor/daisy-embassy/` is a trimmed copy of daisy-embassy 0.3.0. The only intentional source change is `BLOCK_LENGTH = 16` in `src/audio.rs`. Published value is 32.
- Workspace `Cargo.toml` has `[patch.crates-io] daisy-embassy` pointing at that directory. Any `host-daisy` build picks it up, including `breadboard-led`.
- `audio_probe.rs` click 1 is a raw 240 Hz sine (`RAW_TONE_PERIOD_SAMPLES = 200`, level 0.15, `libm::sinf`), engine bypassed. The usual click 1 is a raw triangle. The mode constant is still named `MODE_RAW_TRIANGLE`.
- `host-daisy/Cargo.toml` gained `libm = "0.2.16"` for that sine.
- `README.md` describes this temporary click 1 and the 16-sample patch.

Restore, only when asked:

1. Delete `host-daisy/vendor/daisy-embassy` and the `[patch.crates-io]` block in the workspace `Cargo.toml`.
2. Put click 1 back to the raw triangle and drop the `libm` dependency if nothing else in `host-daisy` uses it.
3. Rebuild and flash `audio-probe` with the commands below.

Flash from the repo root. Hold BOOT, press and release RESET, then release BOOT before `dfu-util`. From WSL, attach the DFU device with `usbipd` first. Always `--release`. For a listen, flash from the PC, then move the Seed back to the powerbank before judging the spectrum.

PowerShell:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo build -p host-daisy --bin audio-probe --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin audio-probe --target thumbv7em-none-eabihf --release -- -O binary audio-probe.bin
dfu-util -a 0 -s 0x08000000:leave -D audio-probe.bin
```

WSL:

```bash
cargo build -p host-daisy --bin audio-probe --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin audio-probe --target thumbv7em-none-eabihf --release -- -O binary audio-probe.bin
dfu-util -a 0 -s 0x08000000:leave -D audio-probe.bin
```

The 16-sample probe already built successfully in WSL on 2026-09-27 (`daisy-embassy` resolved from the vendor path).

## Do not

- Treat the 16-sample patch as the fix.
- Change the engine, the laptop hosts, or the 48 kHz rate for this whine.
- Record new blink counts from the 16-sample firmware into `CONTEXT.md` or `README.md` as voice-capacity results.
- Re-propose items in `REJECTED.md`.
- Start a ground rewire or a 2-sample batch until the author chooses.
