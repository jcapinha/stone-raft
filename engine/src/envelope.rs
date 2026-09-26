/// Envelope stage. For the amp envelope, Idle means the voice is silent and free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Exponential-ish ADSR (0..1) used for amp, filter, and assignable envelopes.
///
/// Times are stored as per-sample coefficients derived from milliseconds.
/// Sustain is a level in 0..1 held until note-off.
#[cfg_attr(test, derive(Clone))]
pub struct Adsr {
    stage: EnvelopeStage,
    level: f32,
    attack_coeff: f32,
    decay_coeff: f32,
    release_coeff: f32,
    sustain: f32,
    sample_rate_hz: f32,
}

impl Adsr {
    const IDLE_LEVEL: f32 = 1.0e-4;
    /// Attack aims slightly above 1 so the asymptotic approach crosses 1 cleanly.
    const ATTACK_TARGET: f32 = 1.01;

    pub fn new(sample_rate_hz: f32) -> Self {
        let mut env = Self {
            stage: EnvelopeStage::Idle,
            level: 0.0,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            sustain: 0.7,
            sample_rate_hz,
        };
        env.set_times_ms(10.0, 100.0, 200.0);
        env
    }

    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.stage != EnvelopeStage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == EnvelopeStage::Release
    }

    pub fn set_sustain(&mut self, sustain: f32) {
        self.sustain = sustain.clamp(0.0, 1.0);
    }

    pub fn set_times_ms(&mut self, attack_ms: f32, decay_ms: f32, release_ms: f32) {
        self.attack_coeff = coeff_from_ms(self.sample_rate_hz, attack_ms);
        self.decay_coeff = coeff_from_ms(self.sample_rate_hz, decay_ms);
        self.release_coeff = coeff_from_ms(self.sample_rate_hz, release_ms);
    }

    pub fn note_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
        // Retrigger from current level so steals and overlaps do not hard-jump to 0.
    }

    pub fn note_off(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
        }
    }

    /// Immediate silence. Used when a mixer instance is turned off.
    pub fn force_idle(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.level = 0.0;
    }

    /// Advances one sample and returns the current level in 0..1.
    #[inline(always)]
    pub fn next_level(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => {
                self.level = 0.0;
            }
            EnvelopeStage::Attack => {
                self.level += (Self::ATTACK_TARGET - self.level) * self.attack_coeff;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.level += (self.sustain - self.level) * self.decay_coeff;
                if (self.level - self.sustain).abs() < Self::IDLE_LEVEL {
                    self.level = self.sustain;
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => {
                self.level = self.sustain;
            }
            EnvelopeStage::Release => {
                self.level += (0.0 - self.level) * self.release_coeff;
                if self.level <= Self::IDLE_LEVEL {
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        self.level
    }

    /// Level after the first of `samples` steps. The stored level is where all of them land.
    ///
    /// One step is `level + (target - level) * coeff`. Steps that stay on the same
    /// target are `target + (level - target) * (1 - coeff)^n`. If attack, decay, or
    /// release crosses inside the block, that stage finishes, the next stage starts,
    /// and the leftover steps use the new target.
    pub(crate) fn advance_block(&mut self, samples: u32) -> f32 {
        if samples == 0 {
            return self.level;
        }
        let block_level = self.project(1).0;
        let (level, stage) = self.project(samples);
        self.level = level;
        self.stage = stage;
        block_level
    }

    fn project(&self, steps: u32) -> (f32, EnvelopeStage) {
        let mut level = self.level;
        let mut stage = self.stage;
        let mut left = steps;
        // Attack can fall into decay, then sustain. Release can fall into idle.
        for _ in 0..4 {
            if left == 0 {
                break;
            }
            match stage {
                EnvelopeStage::Idle => {
                    level = 0.0;
                    break;
                }
                EnvelopeStage::Sustain => {
                    level = self.sustain;
                    break;
                }
                EnvelopeStage::Attack => match approach(
                    level,
                    Self::ATTACK_TARGET,
                    self.attack_coeff,
                    left,
                    StageLimit::Attack,
                ) {
                    ApproachEnd::Crossed(k) => {
                        level = 1.0;
                        stage = EnvelopeStage::Decay;
                        left -= k;
                    }
                    ApproachEnd::Level(next) => {
                        level = next;
                        left = 0;
                    }
                },
                EnvelopeStage::Decay => {
                    let sustain = self.sustain;
                    match approach(level, sustain, self.decay_coeff, left, StageLimit::Decay) {
                        ApproachEnd::Crossed(k) => {
                            level = sustain;
                            stage = EnvelopeStage::Sustain;
                            left -= k;
                        }
                        ApproachEnd::Level(next) => {
                            level = next;
                            left = 0;
                        }
                    }
                }
                EnvelopeStage::Release => {
                    match approach(level, 0.0, self.release_coeff, left, StageLimit::Release) {
                        ApproachEnd::Crossed(k) => {
                            level = 0.0;
                            stage = EnvelopeStage::Idle;
                            left -= k;
                        }
                        ApproachEnd::Level(next) => {
                            level = next;
                            left = 0;
                        }
                    }
                }
            }
        }
        (level, stage)
    }
}

/// Attack, decay, sustain, and release for one ADSR (amp, filter, or assignable).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrTimes {
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
}

impl Default for AdsrTimes {
    fn default() -> Self {
        Self {
            attack_ms: 10.0,
            decay_ms: 100.0,
            sustain: 0.7,
            release_ms: 200.0,
        }
    }
}

impl AdsrTimes {
    pub(crate) fn clamped(self) -> Self {
        Self {
            attack_ms: self.attack_ms.max(0.0),
            decay_ms: self.decay_ms.max(0.0),
            sustain: self.sustain.clamp(0.0, 1.0),
            release_ms: self.release_ms.max(0.0),
        }
    }

    pub(crate) fn apply_to(self, adsr: &mut Adsr) {
        adsr.set_times_ms(self.attack_ms, self.decay_ms, self.release_ms);
        adsr.set_sustain(self.sustain);
    }
}

fn coeff_from_ms(sample_rate_hz: f32, time_ms: f32) -> f32 {
    // Very short times still need a usable coefficient so the stage can finish.
    let time_samples = (time_ms.max(0.1) * 0.001) * sample_rate_hz;
    1.0 - libm::expf(-1.0 / time_samples)
}

#[derive(Clone, Copy)]
enum StageLimit {
    Attack,
    Decay,
    Release,
}

enum ApproachEnd {
    Crossed(u32),
    Level(f32),
}

/// Same thresholds as [`Adsr::next_level`].
fn limit_crossed(limit: StageLimit, level: f32, target: f32) -> bool {
    match limit {
        StageLimit::Attack => level >= 1.0,
        StageLimit::Decay => (level - target).abs() < Adsr::IDLE_LEVEL,
        StageLimit::Release => level <= Adsr::IDLE_LEVEL,
    }
}

/// Level after `steps` toward `target` with no stage change.
fn geom_level(level: f32, target: f32, coeff: f32, steps: u32) -> f32 {
    if steps == 0 || coeff == 0.0 {
        return level;
    }
    if steps == 1 {
        return level + (target - level) * coeff;
    }
    let remain = 1.0 - coeff;
    if remain <= 0.0 {
        return target;
    }
    target + (level - target) * libm::powf(remain, steps as f32)
}

fn approach(level: f32, target: f32, coeff: f32, steps: u32, limit: StageLimit) -> ApproachEnd {
    if steps == 0 {
        return ApproachEnd::Level(level);
    }
    let end = geom_level(level, target, coeff, steps);
    if !limit_crossed(limit, end, target) {
        return ApproachEnd::Level(end);
    }
    let mut lo = 1_u32;
    let mut hi = steps;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if limit_crossed(limit, geom_level(level, target, coeff, mid), target) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    ApproachEnd::Crossed(lo)
}

/// Maps MIDI velocity 0..127 to an amplitude scale with a simple square curve.
/// Softer hits drop more than a straight `velocity / 127` line.
pub fn velocity_to_amp(velocity: u8) -> f32 {
    let v = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    v * v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE_HZ: f32 = 48_000.0;

    fn fast_adsr() -> Adsr {
        let mut env = Adsr::new(SAMPLE_RATE_HZ);
        env.set_times_ms(1.0, 1.0, 50.0);
        env.set_sustain(0.7);
        env
    }

    fn step_until(env: &mut Adsr, stage: EnvelopeStage, max_samples: usize) {
        for _ in 0..max_samples {
            if env.stage() == stage {
                return;
            }
            env.next_level();
        }
        panic!(
            "expected stage {stage:?} within {max_samples} samples, got {:?}",
            env.stage()
        );
    }

    #[test]
    fn adsr_starts_idle() {
        let env = Adsr::new(SAMPLE_RATE_HZ);
        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.level(), 0.0);
        assert!(!env.is_active());
    }

    #[test]
    fn note_on_enters_attack() {
        let mut env = fast_adsr();
        env.note_on();
        assert_eq!(env.stage(), EnvelopeStage::Attack);
        assert!(env.is_active());

        let before = env.level();
        env.next_level();
        assert!(
            env.level() > before,
            "level should rise in attack; before={before} after={}",
            env.level()
        );
    }

    #[test]
    fn attack_reaches_sustain() {
        let mut env = fast_adsr();
        env.note_on();
        // 1 ms attack + 1 ms decay at 48 kHz is ~96 samples; give headroom.
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        assert!((env.level() - 0.7).abs() < 1e-3);
    }

    #[test]
    fn note_off_enters_release() {
        let mut env = fast_adsr();
        env.note_on();
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        env.note_off();
        assert_eq!(env.stage(), EnvelopeStage::Release);
        assert!(env.is_releasing());
    }

    #[test]
    fn release_returns_to_idle() {
        let mut env = fast_adsr();
        env.note_on();
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        env.note_off();
        // Exponential release to IDLE_LEVEL from sustain 0.7 needs ~9 time-constants
        // (50 ms ≈ 2400 samples), so ~22k samples; give headroom.
        step_until(&mut env, EnvelopeStage::Idle, 50_000);
        assert_eq!(env.level(), 0.0);
        assert!(!env.is_active());
    }

    #[test]
    fn force_idle_clears_immediately() {
        let mut env = fast_adsr();
        env.note_on();
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        assert!(env.level() > 0.0);
        env.force_idle();
        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.level(), 0.0);
        assert!(!env.is_active());
    }

    #[test]
    fn retrigger_preserves_level() {
        let mut env = fast_adsr();
        env.note_on();
        for _ in 0..20 {
            env.next_level();
        }
        let mid_attack = env.level();
        assert!(mid_attack > 0.0);
        assert_eq!(env.stage(), EnvelopeStage::Attack);

        env.note_on();
        assert_eq!(env.stage(), EnvelopeStage::Attack);
        assert_eq!(
            env.level(),
            mid_attack,
            "retrigger must not hard-jump level to 0"
        );
    }

    #[test]
    fn velocity_to_amp_curve() {
        let linear_64 = 64.0 / 127.0;
        let curved = velocity_to_amp(64);
        assert!(
            (curved - linear_64 * linear_64).abs() < 1e-5,
            "expected square curve, got {curved}"
        );
        assert!(velocity_to_amp(127) > velocity_to_amp(64));
        assert!(velocity_to_amp(64) > velocity_to_amp(32));
        // Soft velocities drop more than linear: at 64, curve < linear.
        assert!(curved < linear_64);
    }

    fn assert_block_matches(label: &str, env: &Adsr) {
        let mut stepped = env.clone();
        let first = stepped.next_level();
        for _ in 1..32 {
            stepped.next_level();
        }
        let mut closed = env.clone();
        let block_level = closed.advance_block(32);
        let block_error = (block_level - first).abs();
        let level_error = (closed.level() - stepped.level()).abs();
        assert!(
            block_error < 1e-5,
            "{label}: block level {block_level} != first step {first} (delta {block_error})"
        );
        assert!(
            level_error < 1e-5,
            "{label}: stored level {} != step 32 {} (delta {level_error})",
            closed.level(),
            stepped.level()
        );
        assert_eq!(
            closed.stage(),
            stepped.stage(),
            "{label}: stage diverged (closed {:?}, stepped {:?})",
            closed.stage(),
            stepped.stage()
        );
    }

    #[test]
    fn envelope_block_attack_matches_steps() {
        let mut env = Adsr::new(SAMPLE_RATE_HZ);
        env.note_on();
        assert_block_matches("attack", &env);
        let mut end = env.clone();
        for _ in 0..32 {
            end.next_level();
        }
        assert_eq!(end.stage(), EnvelopeStage::Attack);
    }

    #[test]
    fn envelope_block_decay_matches_steps() {
        let mut env = fast_adsr();
        env.set_times_ms(1.0, 80.0, 200.0);
        env.note_on();
        step_until(&mut env, EnvelopeStage::Decay, 5_000);
        assert_block_matches("decay", &env);
        let mut end = env.clone();
        for _ in 0..32 {
            end.next_level();
        }
        assert_eq!(end.stage(), EnvelopeStage::Decay);
    }

    #[test]
    fn envelope_block_sustain_matches_steps() {
        let mut env = fast_adsr();
        env.note_on();
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        assert_block_matches("sustain", &env);
    }

    #[test]
    fn envelope_block_release_matches_steps() {
        let mut env = fast_adsr();
        env.set_times_ms(1.0, 1.0, 200.0);
        env.note_on();
        step_until(&mut env, EnvelopeStage::Sustain, 5_000);
        env.note_off();
        assert_block_matches("release", &env);
        let mut end = env.clone();
        for _ in 0..32 {
            end.next_level();
        }
        assert_eq!(end.stage(), EnvelopeStage::Release);
    }

    #[test]
    fn envelope_block_attack_into_decay_matches_steps() {
        let mut env = Adsr::new(SAMPLE_RATE_HZ);
        env.set_times_ms(0.1, 100.0, 200.0);
        env.set_sustain(0.7);
        env.note_on();
        let mut end = env.clone();
        for _ in 0..32 {
            end.next_level();
        }
        assert_eq!(end.stage(), EnvelopeStage::Decay);
        assert_block_matches("attack into decay", &env);
    }

    fn assert_blocks_track(closed: &mut Adsr, stepped: &mut Adsr, blocks: usize) {
        for block in 0..blocks {
            let block_level = closed.advance_block(32);
            let first = stepped.next_level();
            for _ in 1..32 {
                stepped.next_level();
            }
            let block_error = (block_level - first).abs();
            let level_error = (closed.level() - stepped.level()).abs();
            assert!(
                block_error < 1e-5,
                "block {block}: block level {block_level} != first step {first} (delta {block_error})"
            );
            assert!(
                level_error < 1e-5,
                "block {block}: stored {} != step 32 {} (delta {level_error})",
                closed.level(),
                stepped.level()
            );
            assert_eq!(
                closed.stage(),
                stepped.stage(),
                "block {block}: stage diverged"
            );
        }
    }

    #[test]
    fn envelope_block_crosses_decay_and_release() {
        let mut closed = Adsr::new(SAMPLE_RATE_HZ);
        closed.set_times_ms(0.1, 0.1, 0.1);
        closed.set_sustain(0.4);
        closed.note_on();
        let mut stepped = closed.clone();
        assert_blocks_track(&mut closed, &mut stepped, 4);
        assert_eq!(closed.stage(), EnvelopeStage::Sustain);
        closed.note_off();
        stepped.note_off();
        assert_blocks_track(&mut closed, &mut stepped, 4);
        assert_eq!(closed.stage(), EnvelopeStage::Idle);
        assert_eq!(closed.level(), 0.0);
    }
}
