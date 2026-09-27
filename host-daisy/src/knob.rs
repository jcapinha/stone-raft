//! Cutoff and resonance maps, plus the freeze/track rule for a noisy ADC count.
//!
//! The audio callback never sees raw counts. A pot task publishes one frozen or
//! live count, and these functions turn that count into Hertz or a 0..1 level.

use libm::powf;

/// Full-scale count of the Seed ADC at 16-bit resolution.
pub const ADC_MAX_COUNT: u16 = 65_535;

/// Counts of ADC noise that stay on the last frozen value.
pub const FREEZE_WINDOW: u16 = 12;

/// How long a tracked count must sit still before it freezes again.
pub const STILL_MS: u32 = 40;

pub const CUTOFF_MIN_HZ: f32 = 20.0;
pub const CUTOFF_MAX_HZ: f32 = 16_000.0;

/// Logarithmic cutoff. Full left is 20 Hz, full right is 16 kHz.
/// The middle of the turn is near 570 Hz.
pub fn cutoff_hz(count: u16) -> f32 {
    let t = count as f32 / ADC_MAX_COUNT as f32;
    CUTOFF_MIN_HZ * powf(CUTOFF_MAX_HZ / CUTOFF_MIN_HZ, t)
}

/// Linear resonance, 0 at full left and 1 at full right.
pub fn resonance(count: u16) -> f32 {
    count as f32 / ADC_MAX_COUNT as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Frozen { anchor: u16 },
    Tracking { last: u16, still_ms: u32 },
}

/// Holds the last pot value while the ADC wanders inside [`FREEZE_WINDOW`].
///
/// A turn that leaves the window is followed one count at a time. The window
/// is not a step size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PotTracker {
    phase: Phase,
}

impl PotTracker {
    /// First sample. Audio should publish this count before the callback starts.
    pub fn start(count: u16) -> Self {
        Self {
            phase: Phase::Frozen { anchor: count },
        }
    }

    /// `dt_ms` is how long since the previous sample (about 1 ms on the Seed).
    ///
    /// Returns the count to map. Inside the freeze window that is the anchor,
    /// not the noisy reading.
    pub fn push(&mut self, count: u16, dt_ms: u32) -> u16 {
        match self.phase {
            Phase::Frozen { anchor } => {
                if count.abs_diff(anchor) <= FREEZE_WINDOW {
                    anchor
                } else {
                    self.phase = Phase::Tracking {
                        last: count,
                        still_ms: 0,
                    };
                    count
                }
            }
            Phase::Tracking { last, still_ms } => {
                if count != last {
                    self.phase = Phase::Tracking {
                        last: count,
                        still_ms: 0,
                    };
                    count
                } else {
                    let still_ms = still_ms.saturating_add(dt_ms);
                    if still_ms >= STILL_MS {
                        self.phase = Phase::Frozen { anchor: count };
                    } else {
                        self.phase = Phase::Tracking { last, still_ms };
                    }
                    count
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(actual: f32, expected: f32, tolerance: f32) {
        let delta = (actual - expected).abs();
        assert!(delta <= tolerance, "{actual} vs {expected} (delta {delta})");
    }

    #[test]
    fn cutoff_endpoints_and_middle() {
        near(cutoff_hz(0), 20.0, 0.001);
        near(cutoff_hz(ADC_MAX_COUNT), 16_000.0, 0.05);
        // Geometric middle of 20 Hz and 16 kHz is about 566 Hz.
        near(cutoff_hz(ADC_MAX_COUNT / 2), 566.0, 5.0);
    }

    #[test]
    fn resonance_endpoints() {
        assert_eq!(resonance(0), 0.0);
        near(resonance(ADC_MAX_COUNT), 1.0, 1.0e-6);
    }

    #[test]
    fn freeze_holds_inside_twelve_counts() {
        let mut pot = PotTracker::start(1_000);
        assert_eq!(pot.push(1_000, 1), 1_000);
        assert_eq!(pot.push(1_000 + FREEZE_WINDOW, 1), 1_000);
        assert_eq!(pot.push(1_000 - FREEZE_WINDOW, 1), 1_000);
    }

    #[test]
    fn turn_follows_single_counts() {
        let mut pot = PotTracker::start(1_000);
        assert_eq!(pot.push(1_000 + FREEZE_WINDOW, 1), 1_000);
        assert_eq!(pot.push(1_013, 1), 1_013);
        assert_eq!(pot.push(1_014, 1), 1_014);
        assert_eq!(pot.push(1_015, 1), 1_015);
    }

    #[test]
    fn freezes_again_after_the_still_timer() {
        let mut pot = PotTracker::start(1_000);
        assert_eq!(pot.push(1_020, 1), 1_020);
        assert_eq!(pot.push(1_020, STILL_MS - 1), 1_020);
        assert_eq!(pot.push(1_020, 1), 1_020);
        assert_eq!(pot.push(1_020 + FREEZE_WINDOW, 1), 1_020);
        assert_eq!(pot.push(1_020 + FREEZE_WINDOW + 1, 1), 1_033);
    }
}
