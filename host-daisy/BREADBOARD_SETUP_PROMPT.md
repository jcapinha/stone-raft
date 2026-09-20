# Prompt: walk me through the Daisy breadboard OLED + jack setup

Paste everything below this line into another agent chat.

---

You are helping me wire a Daisy Seed 3 breadboard so firmware `breadboard-led` in the stone-raft repo can show Hello on a cheap 0.96" OLED, play C-E-G through analog audio, and use the existing button/LED. Do not change the firmware unless I ask. Guide me one step at a time, wait for me to confirm, and assume I am not an electronics expert. Give both PowerShell and WSL notes when a command is needed.

## Hardware I already have

- Daisy Seed 3 on a breadboard, USB-C power.
- Existing button: Seed D15 (physical pin 22) with internal pull-up, other side to GND (pin 40). Pin 21 next to D15 is analog 3V3. The button must not go there.
- Existing LED: Seed D24 (physical pin 31) through a series resistor 330 Ω to 1 kΩ to the LED, cathode to GND (pin 40). Leave this wired. Firmware uses it as a gate lamp (on only while a note is held).
- New screen: 0.96" 128x64 OLED, 4-pin I2C, labels GND, VDD, SCK, SDA. SCK is the I2C clock (same as SCL). VDD is 3.3 V. Default address 0x3C. Driver in firmware is SSD1306.
- I need a mono 3.5 mm jack (or clips) for line-level audio. This is not a headphone amp. Use a powered speaker, mixer input, or computer line-in. Passive earbuds can be quiet, distorted, or risky.

Repo pinout drawing: `daisy-seed-3-pinout-diagram.png` at the repo root. Firmware comments in `host-daisy/src/bin/breadboard_led.rs` match this table.

## Pin table (do not skip)

OLED (new wires only):

- OLED GND -> Seed GND pin 40 (same ground as the button)
- OLED VDD -> Seed 3V3 digital pin 38. Never pin 21 (analog 3V3)
- OLED SCK -> Seed D11, physical pin 12 (I2C1 SCL)
- OLED SDA -> Seed D12, physical pin 13 (I2C1 SDA)

Keep:

- Button D15 pin 22 to GND pin 40
- LED D24 pin 31 with series resistor to GND pin 40

Audio jack (new):

- Tip (signal) -> Seed Audio Out 1, physical pin 18
- Sleeve (ground) -> Seed AGND, physical pin 20
- Ring unused for mono. Do not use digital GND pin 40 as the audio sleeve if you can use AGND.

I2C modules usually already have pull-ups. Do not add extra resistors unless the screen never ACKs.

## Flash (firmware is already written)

From the repo root. One-time ARM tools and `dfu-util` are the same as `double-blink` in README.md.

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

## Expected behavior after a good flash

1. Power on: the breadboard LED flashes three times (100 ms on, 100 ms off each time). Then the OLED shows the word Hello for 3 seconds and goes dark. Ignore the button during the boot flash and Hello.
2. First button press: C4, 1 s (LED on), 2 s rest (LED off), E4, rest, G4. Default saw sound at volume 1.0. OLED is a rolling oscilloscope while that runs, then a condensed patch card. Extra presses during the sequence are ignored.
3. Later presses: new random patch at volume 1.0, same C-E-G, scope, then a new card.

## If something is wrong

Work in this order. Do not jump to code changes first.

No Hello, board otherwise alive:

- Recheck OLED GND, VDD on pin 38, SCK on pin 12, SDA on pin 13. Swap SCK/SDA is a common mistake.
- Confirm the module is 4-pin I2C, not 7-pin SPI.
- If the image is present but shifted two pixels, the glass may be SH1106. Tell me that; firmware currently assumes SSD1306.

Hello never appears, no I2C ACK (firmware still plays audio):

- Power and ground first, then clocks. Many modules need 3.3 V, not 5 V.
- Try 4.7 kΩ pull-ups from SDA and SCL to 3.3 V digital only if the module has none.

No sound, Hello works:

- Confirm jack tip is pin 18 and sleeve is AGND pin 20.
- Confirm the speaker/mixer is powered and its volume is up. This is line-level.
- First-press and post-`random` volume is 1.0. Line-out into headphones still sounds quiet.

LED never blinks on press:

- Button must short D15 to GND. LED still needs its resistor. Firmware only lights the LED during the 1 s gates, not during the 2 s rests.

Start by asking me to confirm power is USB-C only, then walk pin 40 ground, then OLED power, then SCK/SDA, then the jack, then flash, then the Hello test.
