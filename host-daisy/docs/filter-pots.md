@TODO: DELETE AFTER USING FOR BREADBOARD DOCUMENTATION ON FITZING AND WIREVIZ
# Bench filter pots

Two B10K pots: cutoff, then resonance. Used by `bench-play` and `breadboard-led`. Pinout: [`daisy-seed-3-pinout-diagram.png`](../../daisy-seed-3-pinout-diagram.png). The full panel still uses the mux in [`BOM.md`](../BOM.md).

## 1. Power off

1. Unplug the USB-C cable from the Seed.
2. Set out two WH148 B10K pots, two 1 kΩ resistors, and two 100 nF ceramics.
3. Set one extra 100 nF aside for the MIDI optocoupler.
4. Leave the jumper from pin 20 (AGND) to pin 40 (DGND) where it is.

A 100 nF ceramic has no stripe and no polarity.

## 2. Rails

1. Find pin 21. That is analog 3.3 V.
2. Find pin 20. That is AGND.

Do not use pin 38 (OLED power), pin 39 (VIN), or pins 18 and 19 (headphones).

## 3. Pot ends

Either outer pin can go to pin 21. The other outer pin goes to pin 20. Leave the center pin free. Both pots can share those two breadboard rows.

1. Wire one outer pin of the cutoff pot to pin 21.
2. Wire the other outer pin of the cutoff pot to pin 20.
3. Wire one outer pin of the resonance pot to pin 21.
4. Wire the other outer pin of the resonance pot to pin 20.

## 4. Cutoff wiper

Pin 23 is on the left edge, USB cable at the bottom of the drawing. Yellow label **A1 / D16**. Pin 21 is two holes above it.

Five holes in one breadboard row are already connected. Two legs in that row touch, with no extra wire.

```text
cutoff center pin → 1 kΩ → pin 23 (A1 / D16)
                              |
                            100 nF
                              |
                         pin 20 (AGND)
```

1. Take a 1 kΩ resistor (bands brown, black, red).
2. Put one leg in the row that holds the cutoff pot's center pin.
3. Put the other leg in the row that holds pin 23.
4. Put one leg of a 100 nF (marked `104`) in that same pin 23 row.
5. Put the other leg in the row that holds pin 20.

## 5. Resonance wiper

Pin 24 is **A2 / D17**, one hole below pin 23. Same chain.

1. Put one leg of the other 1 kΩ in the resonance pot's center-pin row.
2. Put the other leg in the pin 24 row.
3. Put one leg of the other 100 nF in that same pin 24 row.
4. Put the other leg in the pin 20 row.

## 6. Check, then plug in

1. Remove any wire that joins pin 21 straight to pin 20 with no pot between them.
2. Do not add wires to D14, D15, D11, D12, or D24.
3. Plug the USB-C cable back in.

The 1 kΩ and 100 nF only clean the voltage. Firmware does not glide. If a turn feels backwards, swap that pot's two outer wires.

Flash and play from the README: Daisy bench play, or Daisy breadboard play. Both PowerShell and WSL are there. `midi-forward` is PowerShell only. Full left cutoff is about 20 Hz, full right about 16 kHz. Resonance runs from 0 to 1.
