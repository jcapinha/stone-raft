#![cfg_attr(not(test), no_std)]

mod knob;

pub use knob::{
    ADC_MAX_COUNT, CUTOFF_MAX_HZ, CUTOFF_MIN_HZ, FREEZE_WINDOW, PotTracker, STILL_MS, cutoff_hz,
    resonance,
};
