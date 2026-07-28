# Rejected / reversed

Closed doors. Do not re-propose these unless the author explicitly reopens them and updates `CONTEXT.md`.

- **Android phone as the target** — abandoned. The project pivoted to DIY hardware (Daisy Seed). This closes Android app packaging, emulators, sideloading, and any app-store distribution (Google Play or otherwise).
- **Teensy 4.1 as the hardware** — considered and set aside for the Daisy Seed, which has an onboard audio codec and analog inputs (no external DAC to wire).
- **Raspberry Pi as the hardware** — not chosen, but kept as an explicit fallback if embedded Rust on the Daisy proves too hard.
- **FunDSP as an engine dependency** — rejected; it needs std and a heap and will not run on the Daisy. May be used only as a learning reference.
- **USB MIDI on the Daisy** — not used; serial (DIN/TRS) MIDI chosen. USB MIDI remains fine on the laptop for development.
- **Mutex for audio-callback control signals** — rejected in favor of atomics; the audio thread must never risk blocking on a lock.
