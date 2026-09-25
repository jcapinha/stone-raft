//! Shared quarter-wave sine table for oscillator, sub, and sine LFO paths.
//!
//! `libm::sinf` is accurate but too slow to call on every sample inside the
//! Daisy's 32-sample audio callback. Linear interpolation over a quarter cycle
//! stays within a small error and only needs a few multiplies.

const QUARTER_LEN: usize = 256;
const CYCLE_LEN: u32 = (QUARTER_LEN * 4) as u32;

// `.data` is copied into DTCM at boot. A plain static would stay in flash.
#[unsafe(link_section = ".data")]
static SINE_QUARTER: [f32; QUARTER_LEN + 1] = quarter_table();

const fn const_sin(x: f32) -> f32 {
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    let mut n = 1i32;
    while n <= 12 {
        let k = (2 * n) as f32;
        term = -term * x2 / (k * (k + 1.0));
        sum += term;
        n += 1;
    }
    sum
}

const fn quarter_table() -> [f32; QUARTER_LEN + 1] {
    let mut table = [0.0f32; QUARTER_LEN + 1];
    let mut i = 0;
    while i < QUARTER_LEN + 1 {
        let phase = (i as f32) / (QUARTER_LEN as f32);
        table[i] = const_sin(phase * core::f32::consts::FRAC_PI_2);
        i += 1;
    }
    table[0] = 0.0;
    table[QUARTER_LEN] = 1.0;
    table
}

/// Sine of a phase in cycles, where `1.0` is one full turn. Callers keep `phase`
/// in `0..1`. A value that lands exactly on the end of the table wraps to zero.
#[inline(always)]
pub(crate) fn sine_from_phase(phase: f32) -> f32 {
    let phase = if phase >= 1.0 { phase % 1.0 } else { phase };
    let scaled = phase * CYCLE_LEN as f32;
    let mut index = scaled as u32;
    let mut frac = scaled - index as f32;
    if index >= CYCLE_LEN {
        index = 0;
        frac = 0.0;
    }
    let quadrant = (index >> 8) & 3;
    let i = (index & 255) as usize;
    let rising = SINE_QUARTER[i] + (SINE_QUARTER[i + 1] - SINE_QUARTER[i]) * frac;
    let falling = SINE_QUARTER[QUARTER_LEN - i]
        + (SINE_QUARTER[QUARTER_LEN - 1 - i] - SINE_QUARTER[QUARTER_LEN - i]) * frac;
    let sample = match quadrant {
        0 => rising,
        1 => falling,
        2 => -rising,
        3 => -falling,
        _ => 0.0,
    };
    sample.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_stays_close_to_libm() {
        let mut max_error = 0.0f32;
        for step in 0..4096 {
            let phase = step as f32 / 4096.0;
            let approx = sine_from_phase(phase);
            let exact = libm::sinf(core::f32::consts::TAU * phase);
            max_error = max_error.max((approx - exact).abs());
        }
        assert!(
            max_error < 1.0e-4,
            "quarter-wave sine lookup drifted from libm; max error={max_error}"
        );
        assert!(sine_from_phase(0.0).abs() < 1.0e-6);
        assert!((sine_from_phase(0.25) - 1.0).abs() < 1.0e-6);
        assert!(sine_from_phase(0.5).abs() < 1.0e-6);
        assert!((sine_from_phase(0.75) + 1.0).abs() < 1.0e-6);
    }
}
