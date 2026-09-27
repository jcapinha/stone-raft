# Bench filter pots

Two B10K pots for `bench-play`: cutoff, then resonance. About 20 minutes if the ARM tools from `double-blink` are already installed. First flash of this binary is a few minutes longer.

Open the pinout while you work: [`daisy-seed-3-pinout-diagram.png`](../../daisy-seed-3-pinout-diagram.png).

This is the bench slice only. The full panel still uses the mux in [`BOM.md`](../BOM.md). DIN through the optocoupler stays the instrument input.

## 1. Power off and set parts out

About 2 minutes.

1. Unplug the USB-C cable from the Seed.
2. Put these on the bench: two WH148 B10K pots, two 1 kΩ resistors, two 100 nF ceramics.
3. Put one extra 100 nF to the side. That one stays for the MIDI optocoupler.
4. Leave the existing jumper from pin 20 (AGND) to pin 40 (DGND) where it is.

A 100 nF ceramic has no stripe and no polarity. Either end can go to ground.

## 2. Find the two rails

About 1 minute.

1. Find pin 21. That is analog 3.3 V.
2. Find pin 20. That is AGND.

Do not use these for the pots:

- Pin 38 (digital 3.3 V, the OLED)
- Pin 39 (VIN)
- Pin 18 or pin 19 (headphones)

## 3. Tie both pots to those rails

About 5 minutes. Either outer pin of a pot can go to pin 21. The other outer pin goes to pin 20. The center pin is the wiper. Leave every center pin free until the next section.

1. Wire one outer pin of the cutoff pot to pin 21.
2. Wire the other outer pin of the cutoff pot to pin 20.
3. Wire one outer pin of the resonance pot to pin 21.
4. Wire the other outer pin of the resonance pot to pin 20.

Both pots can share the same two breadboard rows for pin 21 and pin 20.

## 4. Cutoff wiper

About 3 minutes.

Use the pot you want as filter frequency. Its two outer pins are already on pin 21 and pin 20. This section uses the center pin only.

Pin 23 is on the left edge of the Seed, with the USB cable at the bottom of the drawing. It is the yellow label **A1 / D16**. Pin 21 is two holes above it.

Five holes in one breadboard row are already connected. Two legs in that same row touch. They do not need a wire between them.

```text
cutoff center pin → 1 kΩ → pin 23 (A1 / D16)
                              |
                            100 nF
                              |
                         pin 20 (AGND)
```

1. Take a 1 kΩ resistor (color bands brown, black, red).
2. Put one leg of that resistor in the breadboard row that holds the cutoff pot's center pin.
3. Put the other leg in the breadboard row that holds Seed pin 23.
4. Put one leg of a 100 nF (marking `104`) in that same pin 23 row, beside the resistor leg.
5. Put the other leg of the 100 nF in the breadboard row that already holds pin 20.

## 5. Resonance wiper

About 3 minutes. Same chain, next pin down.

Pin 24 is the yellow label **A2 / D17**, one hole below pin 23.

```text
resonance center pin → 1 kΩ → pin 24 (A2 / D17)
                                |
                              100 nF
                                |
                           pin 20 (AGND)
```

1. Take the other 1 kΩ resistor (brown, black, red).
2. Put one leg in the breadboard row that holds the resonance pot's center pin.
3. Put the other leg in the breadboard row that holds Seed pin 24.
4. Put one leg of the other 100 nF (`104`) in that same pin 24 row, beside the resistor leg.
5. Put the other leg of that 100 nF in the pin 20 row.

## 6. Check, then plug in

About 1 minute.

1. Look for a wire from pin 21 straight to pin 20 with no pot between them. Remove that wire if you find one. The pot track is what should sit between those two pins.
2. Do not add wires to D14, D15, D11, D12, or D24.
3. Do not move the headphone wires or the USB pins.
4. Plug the USB-C cable back in.

The 1 kΩ and 100 nF only clean the voltage. Firmware does not glide.

## 7. Flash

`bench-play` uses the USB port while it runs. DFU comes back only after you hold BOOT and reset. Pick one shell.

### PowerShell

From the WSL-backed repo. About 2 minutes if `bench-play` was already built, about 5 if this is the first release build.

1. Open PowerShell and run:

```powershell
cd \\wsl$\Ubuntu\home\capinha\audio_experiments\stone-raft
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\stone-raft-target"
$env:CARGO_INCREMENTAL = "0"
```

2. Build:

```powershell
cargo build -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release
```

3. Make the `.bin`:

```powershell
cargo objcopy -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release -- -O binary bench-play.bin
```

4. Hold BOOT, press and release RESET, then release BOOT.
5. Flash:

```powershell
dfu-util -a 0 -s 0x08000000:leave -D bench-play.bin
```

### WSL

Same build, from the repo in Ubuntu. Attach the DFU device with `usbipd` before step 4. `midi-forward` still runs in PowerShell, in the next section.

1. Build:

```bash
cargo build -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release
```

2. Make the `.bin`:

```bash
cargo objcopy -p host-daisy --bin bench-play --target thumbv7em-none-eabihf --release -- -O binary bench-play.bin
```

3. Hold BOOT, press and release RESET, then release BOOT.
4. Attach the DFU device that `usbipd list` shows.
5. Flash:

```bash
dfu-util -a 0 -s 0x08000000:leave -D bench-play.bin
```

Always use `--release`.

## 8. Play

Windows only. Do not run this under WSL. The `stone-raft` port shows up on Windows after `bench-play` leaves the bootloader. About 1 minute.

1. In PowerShell, from the same repo, with the same `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL` as section 7:

```powershell
cargo run -p host-windows --bin midi-forward
```

2. If it prints a numbered list, type the number of the keyboard.
3. Set the keyboard to MIDI channel 1.
4. Play a note. Full left on cutoff is about 20 Hz. Full right is about 16 kHz.
5. Full left on resonance is 0. Full right is 1.

Press Ctrl+C to quit. Held notes get a note-off.

## Later

One action, only if you need it.

1. If a turn feels backwards, swap the two outer wires on that pot. Leave the center wire where it is.
