# stone-raft

A portable Daisy Seed 3 synthesizer written in Rust as a personal first Rust project. See `[CONTEXT.md](CONTEXT.md)` for project decisions and language, and `[REJECTED.md](REJECTED.md)` for closed doors. I use my version of the skill `/grill-with-docs` to stress-test plans and update both files.

## Workspace

- `[engine/](engine)`: `no_std` sound generation shared by every host
- `[host-common/](host-common)`: shared laptop audio, MIDI, commands, and keyboard input
- `[host-wsl/](host-wsl)`: WSL/Linux host for development and optional listening
- `[host-windows/](host-windows)`: native Windows host for reliable audio, MIDI, and key release
- [`host-daisy/`](host-daisy): Seed 3 firmware (`double-blink`, `audio-probe`, `breadboard-led`, `bench-play`). Builds against crates.io `daisy-embassy` 0.3 with `seed3`. `host-daisy/vendor/` is not what Cargo compiles.



## Laptop hosts



### WSL

Install [Rust](https://rustup.rs), ALSA development files, and run:

```bash
sudo apt update && sudo apt install -y pkg-config libasound2-dev
cargo run -p host-wsl
```

WSLg audio can fail with ALSA I/O errors, and WSL usually has no MIDI ports. Use `host-windows` for reliable listening and MIDI; `host-wsl` falls back to the keyboard when MIDI is unavailable.

### Windows

Use PowerShell with the MSVC toolchain. Rust installed in WSL does not count, and GNU/MinGW is unsupported.

One-time setup:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install Rustlang.Rustup
```

Reopen PowerShell, keep the installer's default `x86_64-pc-windows-msvc` host, and confirm it:

```powershell
rustup default stable-x86_64-pc-windows-msvc
rustup show
```

Open the WSL-hosted repository, replacing `<Distro>` with a name such as `Ubuntu`:

```powershell
cd \\wsl$\<Distro>\home\capinha\audio_experiments\stone-raft
```

Build output on `\\wsl$\...` can fail with an “Incorrect function” lock error. Put it on the Windows drive in each new PowerShell session:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo run -p host-windows
```



### Playing

One audio or MIDI device is selected automatically. Multiple devices produce a numbered prompt. MIDI notes reach every enabled engine on the matching listen channel. Engine 1 starts enabled on channel 1; engines 2–4 start disabled on channels 2–4.

Each engine has a fixed 1.75 output calibration before its own `vol` control. `vol 1` is that synth's calibrated full output. When several engines play together, lower their individual volumes if the combined output clips. This behavior is the same in the WSL and PowerShell hosts.

Without MIDI, the current enabled engine uses this C4 keyboard octave. An off engine prints the `on` command needed to enable it.


| Keys                        | Notes                          |
| --------------------------- | ------------------------------ |
| `A W S E D F T G Y H U J K` | C C# D D# E F F# G G# A A# B C |


Press `q` to quit. In MIDI mode, press Enter afterward. WSL terminals do not report key release, so notes use amp release and voice stealing. Native Windows supports hold-to-play.

### Commands

Enter commands directly in MIDI mode. In keyboard mode, press `/`, type one command, and press Enter. Unqualified commands target the current engine. `eng 2 cutoff 800` targets engine 2 once without changing the current engine; `eng2` is invalid.

The four at-pitch oscillator levels are normalized as weights. Sub is additive. Level 0 skips that oscillator's DSP. `wave` selects one at-pitch oscillator and sets all other oscillator and sub levels to 0.


| Command | Meaning |
|---------|---------|
| `eng`; `eng <1..4>` | Show current engine; switch current engine |
| `on`; `off`; `ch <1..16>`; `vol <0..1>` | Enable, disable immediately, route, and set the engine's output volume |
| `show` | Print a replayable qualified patch with all five oscillator levels |
| `cutoff <Hz>`; `res <0..1>` | Filter cutoff and resonance |
| `amp a <ms>`; `amp d <ms>`; `amp s <0..1>`; `amp r <ms>` | Amp ADSR |
| `saw <0..1>`; `sq <0..1>`; `tri <0..1>`; `sin <0..1>` | At-pitch oscillator levels |
| `wave saw|square|triangle|sine` | Solo preset; aliases: `sq`, `tri`, `sin` |
| `pw <0.05..0.95>` | Square pulse width; `0.5` is a classic square |
| `sub <0..1>`; `suboct 1|2` | Additive sine sub level and octave; defaults are `0` and `1` |
| `fenv amt <signed>` | Filter envelope amount in octaves |
| `fenv a <ms>`; `fenv d <ms>`; `fenv s <0..1>`; `fenv r <ms>` | Filter ADSR |
| `asenv dest off|res|pitch|cutoff|pw|amp`; `asenv amt <signed>` | Assignable destination and amount; octaves for pitch/cutoff, linear for resonance, pulse width, and amp; aliases: `resonance`, `pulse`, `pwm` |
| `asenv a <ms>`; `asenv d <ms>`; `asenv s <0..1>`; `asenv r <ms>` | Assignable ADSR |
| `lfo 1` / `lfo 2` dest off|res|pitch|cutoff|pw|amp; amt; rate; wave; retrig | Two assignable LFOs; one shared phase per engine, so every note of that engine reads the same level; bipolar swing around the knob; rate 0.05..20 Hz; retrig defaults off (a new note joins the current level; `retrig on` restarts that shared phase so held notes snap together); waves `sine`, `tri`, `square`, `saw`, `sh` (aliases `triangle`, `sq`, `snh`); `lfo1` is invalid |
| `env copy`; `env link on|off`; `env vel <0..1>` | Copy amp times, link envelope times, and scale extra envelopes by velocity |
| `random` | Randomize subtractive parameters, both LFOs, and volume `0.2..1.0`; always leaves LFO retrig off; keep enabled state and channel |


`show` and `random` print qualified `eng N` lines and do not change enabled state or listen channel.

Example:

```text
eng 2 on
eng 2 cutoff 1200
res 0.4
amp a 5
amp r 400
saw 0.5
sq 0.5
pw 0.2
show
random
```



## Daisy double blink

`double-blink` checks ARM compilation, Seed 3 startup, the onboard LED, and binary conversion without using the synth engine. It produces two 100 ms flashes every two seconds. Keep the commands separate until the manual workflow is proven.

### One-time PowerShell setup

Install the ARM tools:

```powershell
rustup target add thumbv7em-none-eabihf
rustup component add llvm-tools-preview
cargo install cargo-binutils
```

Install [Scoop](https://scoop.sh/) and `dfu-util` from a regular, non-administrator PowerShell:

```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
Invoke-RestMethod -Uri https://get.scoop.sh | Invoke-Expression
scoop install main/dfu-util
dfu-util --version
```

The first command allows local scripts and may request confirmation. `PATH` is where Windows searches for commands. Scoop adds its command folder to `PATH` automatically. If `dfu-util` is not found, reopen PowerShell and retry.

When the repository is under `\\wsl$\...`, set `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL` as shown in the Windows host section before building.

### One-time WSL setup

```bash
rustup target add thumbv7em-none-eabihf
rustup component add llvm-tools-preview
cargo install cargo-binutils
sudo apt update && sudo apt install -y dfu-util
```

Before flashing from WSL, put the Seed into DFU mode and attach the device shown by `usbipd list` using Microsoft's [WSL USB instructions](https://learn.microsoft.com/windows/wsl/connect-usb).

### Build and flash

Run from the repository root in PowerShell or WSL:

```text
cargo build -p host-daisy --bin double-blink --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin double-blink --target thumbv7em-none-eabihf --release -- -O binary double-blink.bin
```

The generated file is disposable. Flashing it replaces the current internal program.

1. Connect the Seed 3 over USB.
2. Hold BOOT, press and release RESET, then release BOOT.
3. Detect and flash the board:

```text
dfu-util --list
dfu-util -a 0 -s 0x08000000:leave -D double-blink.bin
```

The device normally reports USB ID `0483:df11`. If Windows can see it but `dfu-util` cannot open it, use Daisy's [Zadig instructions](https://docs.daisy.audio/tutorials/zadig/) to select WinUSB.

Confirm the double blink after flashing the firmware. If necessary, press RESET, and disconnect and reconnect USB power.

## Daisy audio probe

`audio-probe` separates codec/SAI transport problems from synth engine load. Same TRRS jack, D15 button, and D24 LED as `breadboard-led`. It never talks to the OLED. Instruction cache on, data cache on, codec DMA in RAM_D2 left uncached. The shared sine table is copied into fast RAM at startup. The heavy patch turns both LFO retrigs **on** (engine default is off).

After the long boot flash, each press advances one step, then repeats:

1. Raw triangle (engine bypassed).
2. Default saw, one held note, volume 0.7.
3–6. Deterministic heavy patch on engine 1, one then two, three, four held notes, volume 0.7.
7. Same heavy patch on two engines, one note each, volume 0.35.
8. Four engines, one note each, volume 0.25.
9–10. All four voices on two engines, then three engines. Volume 0.7 divided by engine count.
11. Engines 1–3 stay at four notes each; engine 4 adds one note (B3). Volume 0.7/3. Thirteen heavy voices.
12. All four engines, four voices each. Volume 0.7/4. Sixteen heavy voices.

Full chords are C3, E3, G3, B3. Extra engines load one at a time with a short pause so the queue can drain. Listen channels stay 1 through 4. About two seconds after each press, the LED reports peak 32-frame callback cost:

- One blink: under 50%.
- Two blinks: 50–75%.
- Three blinks: 75% or more.
- Continuous rapid blink: audio interface failed; reset the board.

Last listen (2026-09-27, this sequence, data cache on): click 10 two blinks, click 11 three blinks with audio still coming out, click 12 continuous blink (callback stopped).

PowerShell, from the WSL-backed repository:

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

Before either flash command, hold BOOT, press and release RESET, then release BOOT. WSL also needs the DFU device attached with `usbipd`, as described in the one-time setup above. Always use `--release`.

## Daisy breadboard play

`breadboard-led` runs the synth engine through the Seed 3 codec, plus a 0.96" I2C OLED and the breadboard button/LED. Instruction cache on, data cache **off** (unlike `audio-probe`). Boot flashes the LED three times. OLED uses blocking I2C at 400 kHz with a 200 ms timeout so a missing screen cannot freeze the button. Hello is drawn, stays for 3 seconds, then the screen sleeps, all before audio starts. After that the Seed does not talk to the OLED. Audio uses the daisy-embassy Seed 3 callback. The first button press loops C4, E4, G4 (1 s gate, 2 s rest, then 2 s before the next loop) at volume 1.0. A press during that loop applies `random` and starts the loop again, still at volume 1.0. Cutoff and resonance follow the two bench pots the whole time, including over a random patch. The LED follows each 1 s gate. A continuous rapid blink means audio stopped after an interface error and the board needs a reset.

Audio is line-level on Audio Out 1 and 2 (pins 18 and 19) and AGND (pin 20). Firmware copies the mono mix to both codec channels. A TRRS breakout plus 10 µF caps and 100 Ω resistors can drive headphones. OLED power is 3.3 V digital (pin 38) and GND (pin 40). The pots use analog 3.3 V on pin 21. The OLED and the button do not.

Wiring walkthrough for another agent: [`host-daisy/docs/BREADBOARD_SETUP_PROMPT.md`](host-daisy/docs/BREADBOARD_SETUP_PROMPT.md). Parts, TRRS wiring, and stock: [`host-daisy/BOM.md`](host-daisy/BOM.md).

One-time ARM/`dfu-util` setup is the same as double blink. When the repository is under `\\wsl$\...`, set `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL` as shown in the Windows host section before building. Before flashing from WSL, put the Seed into DFU mode and attach the device shown by `usbipd list`.

Build and flash from the repository root in PowerShell or WSL:

```text
cargo build -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin breadboard-led --target thumbv7em-none-eabihf --release -- -O binary breadboard-led.bin
```

```text
dfu-util --list
dfu-util -a 0 -s 0x08000000:leave -D breadboard-led.bin
```

The generated `.bin` is disposable. Flashing it replaces the current internal program.

## Daisy bench play

`bench-play` is a bench listen path. It does not replace DIN. Audio starts at boot on the same 48 kHz mono headphone path as `breadboard-led` (one mix on both outputs, no OLED, no button arpeggio). D15 and D24 stay unused. Engine 1 listens on MIDI channel 1 at volume 1.0.

Two B10K pots set cutoff and resonance before you play a note. Cutoff is logarithmic, about 20 Hz full left to 16 kHz full right. Resonance is linear, 0 to 1. Wiring: [`host-daisy/docs/filter-pots.md`](host-daisy/docs/filter-pots.md). Unplug USB-C before wiring.

USB MIDI exists only while `bench-play` is running. The device name is `stone-raft`. The same USB-C cable still powers the board. To flash again, hold BOOT and reset so DFU comes back. `breadboard-led` uses these same pots.

One-time ARM/`dfu-util` setup is the same as double blink. When the repository is under `\\wsl$\...`, set `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL` as shown in the Windows host section before building. Before flashing from WSL, put the Seed into DFU mode and attach the device shown by `usbipd list`.

PowerShell:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo build -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release -- -O binary bench-play.bin
dfu-util -a 0 -s 0x08000000:leave -D bench-play.bin
```

WSL:

```bash
cargo build -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release
cargo objcopy -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release -- -O binary bench-play.bin
dfu-util -a 0 -s 0x08000000:leave -D bench-play.bin
```

Before either flash command, hold BOOT, press and release RESET, then release BOOT. Always use `--release`.

### midi-forward

`midi-forward` runs on Windows only. Do not run it under WSL. It copies note on/off from a keyboard to the MIDI port whose name contains `stone-raft`. One input port is selected automatically. Several inputs produce a numbered list. If `stone-raft` is missing, it prints the output ports and exits. Set the keyboard to MIDI channel 1. Press Ctrl+C to quit. Notes it still considers held get a note-off.

PowerShell, after `bench-play` is running (the USB MIDI device is on the Windows side, not inside WSL):

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
cargo run -p host-windows --bin midi-forward
```

From WSL, flash with the commands above, then run `midi-forward` in PowerShell.

## Tests

Same command in WSL or PowerShell. `host-daisy` here is the knob math on the laptop, not the Seed firmware.

```text
cargo test -p engine -p host-common -p host-daisy
```

