# host-daisy bill of materials

Parts for the Daisy Seed 3 breadboard in this repo, plus stock and planned instrument hardware. Pinout: [`daisy-seed-3-pinout-diagram.png`](../daisy-seed-3-pinout-diagram.png). Wiring walkthrough (OLED, button, LED, jack): [`docs/BREADBOARD_SETUP_PROMPT.md`](docs/BREADBOARD_SETUP_PROMPT.md). Bench cutoff and resonance pots: [`docs/filter-pots.md`](docs/filter-pots.md). Flash commands: [`README.md`](../README.md) Daisy sections.

Edit the **On board** column as you add or remove parts. `[x]` means it is on the primary test breadboard now. All `host-daisy` bins (`double-blink`, `audio-probe`, `breadboard-led`) run on that board unless the table says otherwise.

## Assembled breadboard


| On board | Qty | Item | Spec / notes | Seed connection | Used by |
| -------- | --- | ---- | ------------ | --------------- | ------- |
| [x] | 1 | Daisy Seed 3 | STM32H750, TAC5242 codec | USB-C power and DFU | all bins |
| [x] | 1 | Solderless breadboard | Full-size or half-size | Seed inserted | all bins |
| [x] | — | Jumper wires | As needed | — | all bins |
| [x] | 1 | USB-C cable | Data + power | Seed USB | all bins |
| [x] | 1 | Momentary push button | Tactile | D15 (pin 22) to GND (pin 40). Firmware pull-up. Do not use pin 21 (analog 3V3). | `audio-probe`, `breadboard-led` |
| [x] | 1 | LED | 3 mm or 5 mm | D24 (pin 31) → series resistor → LED anode; cathode → GND (pin 40) | `audio-probe`, `breadboard-led` |
| [x] | 1 | Resistor (LED) | 330 Ω–1 kΩ | In series with the LED | `audio-probe`, `breadboard-led` |
| [x] | 1 | 0.96" OLED | 128×64, 4-pin I2C (GND, VDD, SCK, SDA). SSD1306, address 0x3C. Not 7-pin SPI. | GND → pin 40; VDD → **3V3 digital pin 38** (not pin 21); SCK → D11 (pin 12); SDA → D12 (pin 13) | `breadboard-led` |
| [x] | 1 | TRRS 3.5 mm jack breakout | Pads TIP, RING1, RING2, SLEEVE. Accepts normal 3-pole headphones. | See [Headphone path](#headphone-path-trrs) | `audio-probe`, `breadboard-led`, `bench-play` |
| [x] | 2 | Electrolytic capacitor | 10 µF. Stripe (minus) toward the jack. | Pin 18 → cap → 100 Ω → TIP; pin 19 → cap → 100 Ω → RING1 | `audio-probe`, `breadboard-led`, `bench-play` |
| [x] | 2 | Resistor | 100 Ω | After each cap, toward the jack | `audio-probe`, `breadboard-led`, `bench-play` |
| [x] | 1 | AGND ↔ DGND jumper | Required by the Seed datasheet | AGND pin 20 → GND pin 40 | `audio-probe`, `breadboard-led`, `bench-play` |
| [x] | 1 | SLEEVE → AGND jumper | Headphone sleeve return | SLEEVE → AGND pin 20 | `audio-probe`, `breadboard-led`, `bench-play` |
| [x] | 2 | WH148 pot, B10K | Linear 10 kΩ. Bench cutoff and resonance, from the stock kit. Not the full panel. | Outer pins to pin 21 (3V3 analog) and pin 20 (AGND). See [`docs/filter-pots.md`](docs/filter-pots.md). | `bench-play`, `breadboard-led` |
| [x] | 2 | Resistor | 1 kΩ | Series from each wiper to the ADC pin | `bench-play`, `breadboard-led` |
| [x] | 2 | Ceramic capacitor | 100 nF. No stripe, no polarity. | Each ADC pin to AGND pin 20. Do not use the optocoupler’s 100 nF for these. | `bench-play`, `breadboard-led` |


OLED modules usually already have I2C pull-ups. Do not add extra ones unless the screen never answers.

## Headphone path (TRRS)

Line-level from the codec into headphones. Quiet is expected. This is **not** a headphone amplifier. Firmware copies the same mono mix to both codec channels.

Capacitor stripe points toward the jack.

```text
Audio Out 1 (pin 18) → 10 µF → 100 Ω → TIP      (left)
Audio Out 2 (pin 19) → 10 µF → 100 Ω → RING1    (right)
AGND (pin 20)        → RING2                    (ground)
SLEEVE               → AGND (pin 20)            (on the board now)
```

Use AGND (pin 20) for the headphone ground, not pin 40 alone, once the AGND–DGND tie is in place.

If one ear is silent, confirm SLEEVE is also on AGND. If both ears are silent, recheck pin 18/19, cap polarity, the 100 Ω resistors, and RING2.

Do not feed 5 V or pin 21 into the jack. Keep expensive headphones off this jack the first time you try a new flash. `breadboard-led` uses volume 1.0; `audio-probe` uses the volumes in that binary.

**Do not** wire the TDA2822 speaker-amp board into this path. Its 3.5 mm socket is an input. Its screw terminals are speaker outputs and can damage headphones.

## Firmware vs hardware


| Binary | Button D15 | LED D24 | TRRS audio | OLED | Onboard Seed LED |
| ------ | ---------- | ------- | ---------- | ---- | ---------------- |
| `double-blink` | — | — | — | — | yes (two flashes / 2 s) |
| `audio-probe` | yes (test steps) | yes (load / error) | yes | no | boot flash only |
| `breadboard-led` | yes (C–E–G + random) | yes (gate lamp) | yes | yes (Hello, then sleep) | — |
| `bench-play` | — | — | yes | no | — |


## In stock, not on this breadboard

Owned and ready for later wiring. Not part of the current listen path.


| Qty | Item | Use later | Notes |
| --- | ---- | --------- | ----- |
| 1 | 5-pin DIN MIDI socket (DIN-D503 or similar) | Serial MIDI in | Female chassis/PCB socket. Match the cable from the MIDI thru box (DIN vs TRS Type A). |
| 2 | 6N137 optocoupler, DIP-8 | MIDI isolation | One plus a spare. Pin 7 (Enable) must go to 3.3 V or the chip stays mute. Prefer a small protoboard over the breadboard. |
| 1 | 1N4007 diode | MIDI reverse protection | Across the opto LED. 1N4148 is also fine. |
| 1+ | 100 nF ceramic | MIDI supply bypass next to the 6N137 | Markings `100nF`, `0.1µF`, or `104`. Paper `10ONF` is this value. Not the same as 100 µF. Keep one of these for the optocoupler. The two bench filter caps are separate rows above. |
| 8+ | Extra 10 µF electrolytics | Spares / coupling | Two are already on the jack. |
| 5 | EC11 rotary encoder, D-shaft ~15 mm, with push switch | Optional panel controls | 5 pins: A/B/common plus two for the click. Nuts included. Needs D-shaft knobs, not WH148 knobs. |
| 1 kit | WH148 pots, **B** (linear) taper | Panel analog knobs | Use **B5K / B10K / B20K**. Skip 1K and 100K–1M. Cutoff curve can stay in software. Too many pots for the Seed ADC pins: see [Reading many pots](#reading-many-pots). |
| — | SN74HC595N | Extra LEDs / digital **outputs** | 8-bit serial-in, parallel-out shift register. Does **not** read pots or encoders. Keep for engine/status lamps later. |
| 2 | TDA2822M mini amp board | Unused | Speaker amp. Keep off headphones and off this breadboard. |


MIDI also needs about **220 Ω** and **1 kΩ** (drawer resistors are enough) and a **100 nF** on the opto. UART RX on the Seed is typically **D14** (physical pin 15, USART1). Confirm against the pinout drawing before soldering.

## Reading many pots

The Seed 3 has **12 ADC inputs**. Several of those pins (or nearby GPIO) are already claimed: I2C OLED (D11/D12), planned MIDI UART (D14), breadboard button (D15 / A0), breadboard LED (D24 / A9).

The cardboard panel in [`CONTEXT.md`](../CONTEXT.md) is one strip of analog knobs, not one pot per engine. A working count is still more than 12, for example:

- LEVEL
- LFO amount and rate (dest / wave / retrig stay buttons or encoders)
- Osc mix: saw, square, triangle, sine, sub, pulse width (6)
- Filter: cutoff, resonance, EG AMT (3)
- Envelope bank: attack, decay, sustain, release (4)

That is about **16 analog pots**. Direct wiring will not fit.

Use an analog multiplexer, not the 595. Each pot wiper goes to a mux input. The mux common output goes to **one** Seed ADC pin. Four GPIO pins choose which pot is connected. Firmware scans them in a loop. Pot ends go to **3V3 analog pin 21** and AGND (pin 20), not to 5 V.

| Qty to buy | Item | Why |
| ---------- | ---- | --- |
| 2 | **CD74HC4067** (DIP, e.g. CD74HC4067E) | 16-to-1 analog mux. One chip covers ~16 pots (1 ADC + 4 select GPIO). Second chip shares the same four select pins and needs one more ADC, for up to 32 pots or a spare. 3.3 V. Put a 100 nF next to each chip (already in stock). |

An 8-to-1 **CD4051** is a smaller fallback (3 select pins, 8 pots per chip). Two 4051s with shared selects give 16 pots. Prefer the 4067 so one chip matches the current panel count.

The 595 still does not belong on this path. Buttons and EC11 encoders are digital inputs; they need GPIO, an input shift register (**74HC165**), or an I2C expander, not a 595 and not a 4067.

## Planned (not purchased as a set)

From [`CONTEXT.md`](../CONTEXT.md). Quantities follow the cardboard panel mockup, not a finished PCB.

| Qty | Item | Purpose |
| --- | ---- | ------- |
| 2 | CD74HC4067 analog mux | Read the panel pots. See [Reading many pots](#reading-many-pots). |
| 1 | Debug probe (ST-Link or similar) | defmt / step debug. DFU stays the flash path. |
| 1 | Protoboard | MIDI opto circuit, and the mux if it does not stay on the breadboard. |
| — | Extra B10K (or B5K/B20K) pots | Fill the strip: engine pick is buttons; LEVEL; osc mix; filter; envelope bank. See Front panel direction in `CONTEXT.md`. |
| — | Extra tactiles | Engine 1–4, ENV LINK, hold-to-random, as in the mockup. |
| 1 | Optional status OLED | Dest / on / listen channel. Not a live scope. Same 4-pin I2C type as the breadboard if reused. |
| 1 | USB powerbank | Portable 5 V power, USB-C. |

Instrument MIDI in is serial through the optocoupler ([`CONTEXT.md`](../CONTEXT.md)). `bench-play` may expose a temporary USB MIDI device named `stone-raft`. That is not the instrument input ([`REJECTED.md`](../REJECTED.md)).

## Reorder checklist

Use this when restocking the current breadboard.

- [x] Daisy Seed 3 ×1
- [x] Breadboard ×1
- [x] Jumper wire kit
- [x] USB-C cable ×1
- [x] Tactile button ×1
- [x] LED ×1
- [x] 330 Ω–1 kΩ resistor ×1 (LED)
- [x] 0.96" I2C OLED (SSD1306, 4-pin) ×1
- [x] TRRS 3.5 mm breakout ×1
- [x] 10 µF electrolytic ×2
- [x] 100 Ω resistor ×2
- [x] AGND–DGND jumper
- [x] SLEEVE–AGND jumper
- [x] WH148 B10K pot ×2 (bench cutoff and resonance)
- [x] 1 kΩ resistor ×2 (wiper to ADC)
- [x] 100 nF ceramic ×2 (ADC pin to AGND)
- [x] 100 nF ceramic ×1 kept for the MIDI optocoupler (not one of the two bench caps)
