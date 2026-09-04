# stone-raft

A portable Daisy Seed 3 synthesizer written in Rust as a personal first Rust project. See [`CONTEXT.md`](CONTEXT.md) for project decisions and language, and [`REJECTED.md`](REJECTED.md) for closed doors. I use my version of the skill `/grill-with-docs` to stress-test plans and update both files.

## Workspace

- [`engine/`](engine): `no_std` sound generation shared by every host
- [`host-common/`](host-common): shared laptop audio, MIDI, commands, and keyboard input
- [`host-wsl/`](host-wsl): WSL/Linux host for development and optional listening
- [`host-windows/`](host-windows): native Windows host for reliable audio, MIDI, and key release
- [`host-daisy/`](host-daisy): Seed 3 firmware and hardware diagnostics using daisy-embassy

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

Without MIDI, the current enabled engine uses this C4 keyboard octave. An off engine prints the `on` command needed to enable it.

| Keys | Notes |
|------|-------|
| `A W S E D F T G Y H U J K` | C C# D D# E F F# G G# A A# B C |

Press `q` to quit. In MIDI mode, press Enter afterward. WSL terminals do not report key release, so notes use amp release and voice stealing. Native Windows supports hold-to-play.

### Commands

Enter commands directly in MIDI mode. In keyboard mode, press `/`, type one command, and press Enter. Unqualified commands target the current engine. `eng 2 cutoff 800` targets engine 2 once without changing the current engine; `eng2` is invalid.

The four at-pitch oscillator levels are normalized as weights. Sub is additive. Level 0 skips that oscillator's DSP. `wave` selects one at-pitch oscillator and sets all other oscillator and sub levels to 0.

| Command | Meaning |
|---------|---------|
| `eng`; `eng <1..4>` | Show current engine; switch current engine |
| `on`; `off`; `ch <1..16>`; `vol <0..1>` | Enable, disable immediately, route, and set instance volume |
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
| `lfo 1` / `lfo 2` dest off|res|pitch|cutoff|pw|amp; amt; rate; wave; retrig | Two assignable LFOs; bipolar swing around the knob; rate 0.05..20 Hz; retrig defaults on; waves `sine`, `tri`, `square`, `saw`, `sh` (aliases `triangle`, `sq`, `snh`); `lfo1` is invalid |
| `env copy`; `env link on|off`; `env vel <0..1>` | Copy amp times, link envelope times, and scale extra envelopes by velocity |
| `random` | Randomize subtractive parameters, both LFOs, and volume `0.2..1.0`; keep enabled state and channel |

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

## Tests

Same command in WSL or PowerShell:

```text
cargo test -p engine -p host-common
```
