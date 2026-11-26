//! Cutoff and resonance from an ADC count. The freeze window is noise, not a step size.

use libm::powf;

/// Full-scale count of the Seed ADC at 16-bit resolution.
pub const ADC_MAX_COUNT: u16 = 65_535;

/// Counts of ADC noise that stay on the last frozen value.
pub const FREEZE_WINDOW: u16 = 12;

/// How long a tracked count must sit still before it freezes again.
pub const STILL_MS: u32 = 40;

pub const CUTOFF_MIN_HZ: f32 = 20.0;
pub const CUTOFF_MAX_HZ: f32 = 16_000.0;

/// Logarithmic cutoff, 20 Hz at count 0 and 16 kHz at full scale.
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

/// Frozen inside [`FREEZE_WINDOW`] of the anchor. A turn past that follows every count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PotTracker {
    phase: Phase,
}

impl PotTracker {
    pub fn start(count: u16) -> Self {
        Self {
            phase: Phase::Frozen { anchor: count },
        }
    }

    /// `dt_ms` is the time since the previous sample. Inside the window this returns the anchor.
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
