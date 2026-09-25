#![no_std]
#![no_main]

//! Permanent Seed 3 audio isolation probe. It uses D15 for the button, D24 for
//! status, and the normal codec outputs. It never initializes the OLED.
//!
//! Each button press advances through:
//! 1. Raw triangle, with the synth engine bypassed.
//! 2. The default one-saw engine patch with one held note.
//! 3. A deterministic heavy patch with one held note.
//! 4. The same heavy patch with two, three, then four held notes.
//!
//! After a short settling period, the LED reports the peak 32-frame callback
//! cost: one blink is under 50% of the available cycles, two is 50-75%, and
//! three is 75% or more. A continuous rapid blink means the audio interface
//! failed; audio has stopped and the board must be reset.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use cortex_m::peripheral::{DWT, Peripherals};
use daisy_embassy::audio::AudioPeripherals;
use daisy_embassy::hal::gpio::{Input, Level, Output, Pull, Speed};
use daisy_embassy::{hal, new_daisy_board};
use embassy_executor::Spawner;
use embassy_time::Timer;
use engine::{
    AdsrTimes, AssignableDest, EngineParams, InstanceEvent, LfoParams, LfoWave, Mixer, MixerEvent,
    SubOctaves, patch_events,
};
use heapless::spsc::{Consumer, Producer, Queue};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const SAMPLE_RATE_HZ: f32 = 48_000.0;
const ENGINE_INSTANCE: u8 = 1;
const LISTEN_CHANNEL: u8 = 1;
const NOTE_VELOCITY: u8 = 100;
const DEFAULT_NOTE: u8 = 60;
const HEAVY_NOTES: [u8; 4] = [48, 52, 55, 59];
const DEFAULT_VOLUME: f32 = 0.7;
const HEAVY_VOLUME: f32 = 0.7;

const MODE_SILENT: u8 = 0;
const MODE_RAW_TRIANGLE: u8 = 1;
const MODE_DEFAULT_ENGINE: u8 = 2;
const MODE_HEAVY_ENGINE: u8 = 3;

const CALLBACK_CYCLES: u32 = 320_000;
const HALF_BUDGET_CYCLES: u32 = CALLBACK_CYCLES / 2;
const THREE_QUARTER_BUDGET_CYCLES: u32 = CALLBACK_CYCLES * 3 / 4;

const RAW_TRIANGLE_PERIOD_SAMPLES: u32 = 200;
const RAW_TRIANGLE_LEVEL: f32 = 0.15;
const EVENT_QUEUE_CAP: usize = 64;
const CONFIG_SETTLE_MS: u64 = 250;
const MEASURE_MS: u64 = 1_500;
const CONFIG_SILENCE_MS: u64 = 20;
const BOOT_FLASH_MS: u64 = 300;
const RESULT_BLINK_MS: u64 = 180;
const RESULT_GAP_MS: u64 = 400;
const ERROR_BLINK_MS: u64 = 75;
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 10;

static MIXER: StaticCell<Mixer> = StaticCell::new();
static EVENT_QUEUE: StaticCell<Queue<MixerEvent, EVENT_QUEUE_CAP>> = StaticCell::new();
static TEST_MODE: AtomicU8 = AtomicU8::new(MODE_SILENT);
static RESULT_STATUS: AtomicU8 = AtomicU8::new(0);
static AUDIO_ERROR: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
enum ProbeStep {
    RawTriangle,
    DefaultEngine,
    HeavyOne,
    HeavyTwo,
    HeavyThree,
    HeavyFour,
}

impl ProbeStep {
    fn next(self) -> Self {
        match self {
            ProbeStep::RawTriangle => ProbeStep::DefaultEngine,
            ProbeStep::DefaultEngine => ProbeStep::HeavyOne,
            ProbeStep::HeavyOne => ProbeStep::HeavyTwo,
            ProbeStep::HeavyTwo => ProbeStep::HeavyThree,
            ProbeStep::HeavyThree => ProbeStep::HeavyFour,
            ProbeStep::HeavyFour => ProbeStep::RawTriangle,
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(p);

    let mut core = Peripherals::take().unwrap();
    // Instruction cache on, data cache off. Flash fetches were the slow part.
    // The data cache would also cover the codec DMA buffers in RAM_D2.
    core.SCB.enable_icache();
    core.DCB.enable_trace();
    DWT::unlock();
    core.DWT.set_cycle_count(0);
    core.DWT.enable_cycle_counter();

    let button = Input::new(board.pins.d15, Pull::Up);
    let mut led = Output::new(board.pins.d24, Level::Low, Speed::Low);
    led.set_high();
    Timer::after_millis(BOOT_FLASH_MS).await;
    led.set_low();

    let queue = EVENT_QUEUE.init(Queue::new());
    let (producer, consumer) = queue.split();
    spawner.spawn(audio_loop(board.audio_peripherals, consumer).unwrap());

    probe_ui(button, led, producer).await;
}

#[embassy_executor::task]
async fn audio_loop(audio: AudioPeripherals<'static>, mut consumer: Consumer<'static, MixerEvent>) {
    let mixer = MIXER.init(Mixer::new(SAMPLE_RATE_HZ));
    let idle = audio.prepare_interface(Default::default()).await;
    let Ok(mut interface) = idle.start_interface().await else {
        AUDIO_ERROR.store(true, Ordering::Relaxed);
        return;
    };
    let mut triangle_position = 0u32;

    let result = interface
        .start_callback(|_input, output| {
            let started = DWT::cycle_count();

            while let Some(event) = consumer.dequeue() {
                mixer.apply(event);
            }

            let mode = TEST_MODE.load(Ordering::Relaxed);
            match mode {
                MODE_RAW_TRIANGLE => {
                    for frame in output.chunks_exact_mut(2) {
                        let sample = raw_triangle(triangle_position) * RAW_TRIANGLE_LEVEL;
                        let bits = f32_to_u24(sample);
                        frame[0] = bits;
                        frame[1] = bits;
                        triangle_position = triangle_position.wrapping_add(1);
                    }
                }
                MODE_DEFAULT_ENGINE | MODE_HEAVY_ENGINE => {
                    for frame in output.chunks_exact_mut(2) {
                        let bits = f32_to_u24(mixer.next_sample());
                        frame[0] = bits;
                        frame[1] = bits;
                    }
                }
                MODE_SILENT => output.fill(0),
                _ => output.fill(0),
            }

            if mode != MODE_SILENT {
                let elapsed = DWT::cycle_count().wrapping_sub(started);
                record_result(classify_cycles(elapsed));
            }
        })
        .await;

    if result.is_err() {
        TEST_MODE.store(MODE_SILENT, Ordering::Relaxed);
        AUDIO_ERROR.store(true, Ordering::Relaxed);
    }
}

async fn probe_ui(
    button: Input<'static>,
    mut led: Output<'static>,
    mut producer: Producer<'static, MixerEvent>,
) -> ! {
    let mut step = ProbeStep::HeavyFour;

    loop {
        if !wait_for_press(&button).await || !wait_for_release(&button).await {
            error_blink(&mut led).await;
        }

        step = step.next();
        RESULT_STATUS.store(0, Ordering::Relaxed);
        if !configure_step(step, &mut producer).await {
            error_blink(&mut led).await;
        }

        if !wait_unless_error(CONFIG_SETTLE_MS).await {
            error_blink(&mut led).await;
        }
        RESULT_STATUS.store(0, Ordering::Relaxed);
        if !wait_unless_error(MEASURE_MS).await {
            error_blink(&mut led).await;
        }

        if !show_result(&mut led).await {
            error_blink(&mut led).await;
        }
    }
}

async fn configure_step(step: ProbeStep, producer: &mut Producer<'static, MixerEvent>) -> bool {
    match step {
        ProbeStep::RawTriangle => {
            TEST_MODE.store(MODE_RAW_TRIANGLE, Ordering::Relaxed);
            enqueue(producer, enabled_event(false))
        }
        ProbeStep::DefaultEngine => {
            TEST_MODE.store(MODE_SILENT, Ordering::Relaxed);
            if !enqueue(producer, enabled_event(false))
                || !enqueue_patch(producer, &EngineParams::default(), DEFAULT_VOLUME)
                || !enqueue(producer, enabled_event(true))
                || !enqueue(producer, note_on(DEFAULT_NOTE))
            {
                return false;
            }
            if !wait_unless_error(CONFIG_SILENCE_MS).await {
                return false;
            }
            TEST_MODE.store(MODE_DEFAULT_ENGINE, Ordering::Relaxed);
            true
        }
        ProbeStep::HeavyOne => {
            TEST_MODE.store(MODE_SILENT, Ordering::Relaxed);
            if !enqueue(producer, enabled_event(false))
                || !enqueue_patch(producer, &heavy_params(), HEAVY_VOLUME)
                || !enqueue(producer, enabled_event(true))
                || !enqueue(producer, note_on(HEAVY_NOTES[0]))
            {
                return false;
            }
            if !wait_unless_error(CONFIG_SILENCE_MS).await {
                return false;
            }
            TEST_MODE.store(MODE_HEAVY_ENGINE, Ordering::Relaxed);
            true
        }
        ProbeStep::HeavyTwo => enqueue(producer, note_on(HEAVY_NOTES[1])),
        ProbeStep::HeavyThree => enqueue(producer, note_on(HEAVY_NOTES[2])),
        ProbeStep::HeavyFour => enqueue(producer, note_on(HEAVY_NOTES[3])),
    }
}

fn heavy_params() -> EngineParams {
    let mut params = EngineParams::default();
    params.saw_vol = 1.0;
    params.square_vol = 1.0;
    params.triangle_vol = 1.0;
    params.sine_vol = 1.0;
    params.pulse_width = 0.35;
    params.sub_vol = 0.5;
    params.sub_octaves = SubOctaves::One;
    params.cutoff_hz = 600.0;
    params.resonance = 0.65;
    params.amp_env = AdsrTimes {
        attack_ms: 5.0,
        decay_ms: 120.0,
        sustain: 0.8,
        release_ms: 300.0,
    };
    params.filter_env = AdsrTimes {
        attack_ms: 50.0,
        decay_ms: 1_500.0,
        sustain: 0.25,
        release_ms: 500.0,
    };
    params.filter_env_amount = 4.0;
    params.assignable_env = AdsrTimes {
        attack_ms: 500.0,
        decay_ms: 5_000.0,
        sustain: 0.4,
        release_ms: 500.0,
    };
    params.assignable_dest = AssignableDest::Resonance;
    params.assignable_amount = 0.5;
    params.lfos = [
        LfoParams {
            dest: AssignableDest::Cutoff,
            amount: 1.5,
            rate_hz: 1.3,
            wave: LfoWave::Sine,
            retrigger: true,
        },
        LfoParams {
            dest: AssignableDest::Pitch,
            amount: 0.08,
            rate_hz: 0.7,
            wave: LfoWave::Sine,
            retrigger: true,
        },
    ];
    params
}

fn enqueue_patch(
    producer: &mut Producer<'static, MixerEvent>,
    params: &EngineParams,
    volume: f32,
) -> bool {
    for event in patch_events(ENGINE_INSTANCE, params, volume).as_slice() {
        if !enqueue(producer, *event) {
            return false;
        }
    }
    true
}

fn enabled_event(on: bool) -> MixerEvent {
    MixerEvent::ToInstance {
        instance: ENGINE_INSTANCE,
        event: InstanceEvent::SetEnabled { on },
    }
}

fn note_on(note: u8) -> MixerEvent {
    MixerEvent::MidiNoteOn {
        channel: LISTEN_CHANNEL,
        note,
        velocity: NOTE_VELOCITY,
    }
}

fn enqueue(producer: &mut Producer<'static, MixerEvent>, event: MixerEvent) -> bool {
    if producer.enqueue(event).is_err() {
        AUDIO_ERROR.store(true, Ordering::Relaxed);
        false
    } else {
        true
    }
}

fn classify_cycles(cycles: u32) -> u8 {
    if cycles < HALF_BUDGET_CYCLES {
        1
    } else if cycles < THREE_QUARTER_BUDGET_CYCLES {
        2
    } else {
        3
    }
}

fn record_result(result: u8) {
    let mut current = RESULT_STATUS.load(Ordering::Relaxed);
    while result > current {
        match RESULT_STATUS.compare_exchange_weak(
            current,
            result,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

fn raw_triangle(position: u32) -> f32 {
    let position = position % RAW_TRIANGLE_PERIOD_SAMPLES;
    if position <= RAW_TRIANGLE_PERIOD_SAMPLES / 2 {
        position as f32 * 4.0 / RAW_TRIANGLE_PERIOD_SAMPLES as f32 - 1.0
    } else {
        let falling_position = position - RAW_TRIANGLE_PERIOD_SAMPLES / 2;
        falling_position as f32 * -4.0 / RAW_TRIANGLE_PERIOD_SAMPLES as f32 + 1.0
    }
}

async fn show_result(led: &mut Output<'static>) -> bool {
    let count = RESULT_STATUS.load(Ordering::Relaxed).clamp(1, 3);
    for _ in 0..count {
        led.set_high();
        if !wait_unless_error(RESULT_BLINK_MS).await {
            return false;
        }
        led.set_low();
        if !wait_unless_error(RESULT_BLINK_MS).await {
            return false;
        }
    }
    wait_unless_error(RESULT_GAP_MS).await
}

async fn wait_for_press(button: &Input<'_>) -> bool {
    loop {
        if AUDIO_ERROR.load(Ordering::Relaxed) {
            return false;
        }
        if button.is_low() {
            if !wait_unless_error(DEBOUNCE_MS).await {
                return false;
            }
            if button.is_low() {
                return true;
            }
        }
        Timer::after_millis(POLL_MS).await;
    }
}

async fn wait_for_release(button: &Input<'_>) -> bool {
    while button.is_low() {
        if !wait_unless_error(POLL_MS).await {
            return false;
        }
    }
    wait_unless_error(DEBOUNCE_MS).await
}

async fn wait_unless_error(duration_ms: u64) -> bool {
    let mut remaining = duration_ms;
    while remaining > 0 {
        if AUDIO_ERROR.load(Ordering::Relaxed) {
            return false;
        }
        let sleep_ms = remaining.min(POLL_MS);
        Timer::after_millis(sleep_ms).await;
        remaining -= sleep_ms;
    }
    !AUDIO_ERROR.load(Ordering::Relaxed)
}

async fn error_blink(led: &mut Output<'static>) -> ! {
    loop {
        led.set_high();
        Timer::after_millis(ERROR_BLINK_MS).await;
        led.set_low();
        Timer::after_millis(ERROR_BLINK_MS).await;
    }
}

fn f32_to_u24(x: f32) -> u32 {
    let x = x * 8_388_607.0;
    let x = x.clamp(-8_388_608.0, 8_388_607.0);
    (x as i32) as u32
}
