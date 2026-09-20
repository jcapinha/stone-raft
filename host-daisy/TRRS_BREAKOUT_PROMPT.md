# Prompt: wire the TRRS breakout to the Daisy Seed for breadboard headphones

Paste everything below this line into another agent chat.

---

You are helping me connect a 3.5 mm TRRS breakout to a Daisy Seed 3 on a breadboard so I can listen with headphones. Firmware `breadboard-led` already exists and already copies the mono mix to both codec channels. Do not change the firmware unless I ask. Do not start MIDI, encoders, pots, or a speaker amp in this session. Guide me one step at a time, wait for me to confirm, and assume I am not an electronics expert. Give both PowerShell and WSL notes when a command is needed.

Read `CONTEXT.md` and `REJECTED.md` at the repo root before suggesting architecture changes. USB MIDI on the Daisy is rejected. Serial MIDI is a later session.

## Goal

Safe headphone listen from Seed analog audio, using parts I already have. Quiet is OK. A TDA2822 speaker amp is **not** in this path.

## What is already working

- Daisy Seed 3 on a breadboard, USB-C power.
- Firmware: `host-daisy` binary `breadboard-led` (`src/bin/breadboard_led.rs`).
- Button: D15 (physical pin 22) internal pull-up, other side to GND pin 40. Pin 21 next to D15 is analog 3V3. The button must not go there.
- LED: D24 (physical pin 31) through 330 Ω to 1 kΩ, cathode to GND pin 40. Gate lamp while a note is held.
- OLED: 0.96" 128x64 I2C SSD1306. GND pin 40, VDD **3V3 digital pin 38** (never pin 21), SCK/SCL D11 pin 12, SDA D12 pin 13.
- Audio callback writes the same sample to left and right (`chunk[0]` and `chunk[1]`). Start volume is 1.0. First button press plays C4-E4-G4. Later presses run `random` (volume stays 1.0) then the same arpeggio.

Repo pinout: `daisy-seed-3-pinout-diagram.png`. Match physical pin numbers to that drawing.

Leave OLED, button, and LED wired. Add only the jack path.

## Hardware for this session

I have:

- Red **TRRS breakout** (4 pads: TIP, RING1, RING2, SLEEVE) plus a 4-pin header to plug into the breadboard. Phone headsets use four contacts. Normal headphones use three. This jack still accepts headphones.
- 10 µF electrolytic capacitors (need two).
- 100 Ω resistors (need two).
- Headphones. Start with cheap ones. Firmware volume 1.0 on first press and after `random`.

I am **not** using the TDA2822 board here. Its 3.5 mm hole is an **input**. Its screws are **speaker** outputs. Headphones on those screws can get damaged.

## Wiring (do this in this order)

Tie **AGND pin 20 to DGND pin 40** with one jumper if that link is not already there. The Seed datasheet requires it.

Capacitors: electrolytic. The stripe (minus) points **toward the jack**, away from the Seed.

For each ear:

1. Seed **Audio Out 1**, physical pin **18** → 10 µF → 100 Ω → breakout **TIP**
2. Seed **Audio Out 2**, physical pin **19** → 10 µF → 100 Ω → breakout **RING1**
3. Seed **AGND**, physical pin **20** → breakout **RING2**
4. Leave **SLEEVE** unconnected at first

If one ear is silent after a known-good arpeggio, also join **SLEEVE** to AGND.

Do not use digital GND pin 40 as the headphone ground if AGND pin 20 is available. Do not feed 5 V or pin 21 into the jack.

## Flash (only if the current firmware is not already `breadboard-led`)

From the repo root. One-time ARM tools and `dfu-util` match `README.md` (double blink / breadboard play).

PowerShell (repo under `\\wsl$\...`):

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo build -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release -- -O binary breadboard-led.bin
```

WSL:

```bash
cargo build -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release -- -O binary breadboard-led.bin
```

Before WSL `dfu-util`, put the Seed in DFU mode and attach it with `usbipd` (Microsoft WSL USB docs).

DFU:

1. USB-C connected.
2. Hold BOOT, press and release RESET, release BOOT.
3. `dfu-util --list` then `dfu-util -a 0 -s 0x08000000:leave -D breadboard-led.bin`
4. USB ID is usually `0483:df11`. If Windows sees DFU but `dfu-util` cannot open it, use Daisy Zadig/WinUSB instructions.

## Expected listen test

1. Headphones in the breakout. Volume on the headphones (if any) mid, not max.
2. Power on: breadboard LED flashes three times, OLED Hello for 3 s.
3. One button press: C-E-G in both ears. Line-out into headphones is still quieter than a phone. LED on during each 1 s note.
4. If silent: unplug headphones, recheck pin 18/19, cap polarity, 100 Ω in series, RING2 to AGND, AGND tied to pin 40.

## If something is wrong

No firmware changes first.

No sound, OLED still works:

- Tip and RING1 must see the two audio out pins through caps and resistors.
- RING2 must be AGND pin 20.
- Try SLEEVE to AGND only after that.

Sound in one ear only:

- Pin 19 path, or SLEEVE vs RING2. Firmware already duplicates mono to both channels.

Too loud or harsh:

- Unplug. First-press volume is 1.0. Do not use the speaker amp. Wait for the arpeggio to finish.

OLED or button broke after adding the jack:

- Jack ground stole pin 40 without AGND. Restore OLED VDD on pin 38. Button still D15 to pin 40.

## Out of scope this session

MIDI DIN, 6N137, 1N4007, 100 nF (`104` / `10ONF` paper typo), encoders, WH148 pots, TDA2822, panel layout.

Start by asking me to confirm USB-C power, existing OLED/button/LED still in place, then AGND-to-DGND, then the two capacitor paths, then RING2, then plug headphones and press the button.
