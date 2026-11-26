#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

//! Breadboard synth check. USB powers the Seed.
//!
//! Wiring (see `daisy-seed-3-pinout-diagram.png` at the repo root):
//! - D11 = physical pin 12. OLED I2C SCL (module SCK).
//! - D12 = physical pin 13. OLED I2C SDA.
//! - D15 = physical pin 22. Button input with internal pull-up. Press shorts this pin to GND.
//! - D24 = physical pin 31. LED output; high turns the LED on during each 1 s gate. Series resistor 330 Ω to 1 kΩ required
//! - Audio Out 1 = physical pin 18. TRRS TIP. Same mono mix copied to both codec channels.
//! - Audio Out 2 = physical pin 19. TRRS RING1.
//! - AGND = physical pin 20. TRRS RING2 (and SLEEVE if needed). Tie to DGND pin 40.
//! - 3V3 analog = physical pin 21. Pot ends, shared with AGND pin 20. Not for the OLED or the button.
//! - Cutoff wiper, through 1 kΩ: A1 / D16, physical pin 23. 100 nF from that pin to AGND.
//! - Resonance wiper, through 1 kΩ: A2 / D17, physical pin 24. Its own 100 nF to AGND.
//! - 3V3 digital = physical pin 38. OLED VDD.
//! - GND = physical pin 40. Shared ground for OLED, LED cathode, and button.
//!
//! Boot flashes the breadboard LED three times. OLED uses blocking I2C at 400 kHz
//! with a 200 ms timeout so a missing screen cannot freeze the button. Hello is
//! drawn, held for 3 s, then the screen sleeps, all before audio starts. After
//! that the Seed does not talk to the OLED. Volume is 1.0 on first press and
//! after `random`. The first press loops C4-E4-G4 (1 s gate, 2 s rest, then 2 s
//! before the next loop). A press during that loop runs `random` and starts the
//! loop again. The two pots replace cutoff and resonance on every audio callback,
//! including inside a random patch. Audio uses the daisy-embassy Seed 3 callback
//! (TX and RX paced to the codec). An SAI error stops audio and changes the LED
//! to a continuous rapid blink until reset.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cortex_m::peripheral::Peripherals;
use daisy_embassy::audio::AudioPeripherals;
use daisy_embassy::hal::adc::{Adc, SampleTime};
use daisy_embassy::hal::gpio::{Input, Level, Output, Pull, Speed};
use daisy_embassy::hal::i2c::{self, I2c};
use daisy_embassy::hal::time::Hertz;
use daisy_embassy::{hal, new_daisy_board};
use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::FONT_5X8;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use engine::{ControlEvent, InstanceEvent, Mixer, MixerEvent, patch_events, random_patch};
use heapless::spsc::{Consumer, Producer, Queue};
use host_daisy::{PotTracker, cutoff_hz, resonance};
use rand::SeedableRng;
use rand::rngs::SmallRng;
use ssd1306::mode::{BufferedGraphicsMode, DisplayConfig};
use ssd1306::prelude::{DisplayRotation, DisplaySize128x64};
use ssd1306::{I2CDisplayInterface, Ssd1306};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const SAMPLE_RATE_HZ: f32 = 48_000.0;
const START_VOLUME: f32 = 1.0;
const NOTE_VELOCITY: u8 = 100;
const LISTEN_CHANNEL: u8 = 1;
const ENGINE_INSTANCE: u8 = 1;
const NOTES: [u8; 3] = [60, 64, 67];
const GATE_MS: u64 = 1_000;
const REST_MS: u64 = 2_000;
const LOOP_GAP_MS: u64 = 2_000;
const POT_PERIOD_MS: u32 = 1;
const ADC_SAMPLE_TIME: SampleTime = SampleTime::CYCLES810_5;
const HELLO_MS: u64 = 3_000;
const OLED_INIT_TIMEOUT_MS: u64 = 200;
const BOOT_FLASH_MS: u64 = 100;
const ERROR_BLINK_MS: u64 = 75;
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 10;
const EVENT_QUEUE_CAP: usize = 64;

type OledI2c = I2c<'static, daisy_embassy::hal::mode::Blocking, i2c::mode::Master>;
type OledDisplay = Ssd1306<
    ssd1306::prelude::I2CInterface<OledI2c>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

static MIXER: StaticCell<Mixer> = StaticCell::new();
static EVENT_QUEUE: StaticCell<Queue<MixerEvent, EVENT_QUEUE_CAP>> = StaticCell::new();
static AUDIO_ERROR: AtomicBool = AtomicBool::new(false);
static GATE_LED_ON: AtomicBool = AtomicBool::new(false);
static CUTOFF_BITS: AtomicU32 = AtomicU32::new(0);
static RESONANCE_BITS: AtomicU32 = AtomicU32::new(0);

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg_attr(target_os = "none", embassy_executor::main)]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
async fn run(spawner: Spawner) {
    let p = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(p);

    let mut core = Peripherals::take().unwrap();
    // Instruction cache on, data cache off.
    core.SCB.enable_icache();

    let button = Input::new(board.pins.d15, Pull::Up);
    let mut led = Output::new(board.pins.d24, Level::Low, Speed::Low);
    boot_flash(&mut led).await;

    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = Hertz::khz(400);
    i2c_config.timeout = Duration::from_millis(OLED_INIT_TIMEOUT_MS);
    i2c_config.sda_pullup = true;
    i2c_config.scl_pullup = true;
    let i2c = I2c::new_blocking(p.I2C1, board.pins.d11, board.pins.d12, i2c_config);
    let mut display = init_oled(i2c);
    if let Some(display) = display.as_mut() {
        show_hello(display);
    }

    let queue = EVENT_QUEUE.init(Queue::new());
    let (producer, consumer) = queue.split();

    if let Some(display) = display.as_mut() {
        Timer::after_millis(HELLO_MS).await;
        let _ = display.clear(BinaryColor::Off);
        oled_flush(display);
        oled_set_on(display, false);
    }

    // 16-bit is the H7 ADC reset resolution, matching `ADC_MAX_COUNT`.
    let mut adc = Adc::new(p.ADC1);
    let mut cutoff_pin = board.pins.d16;
    let mut resonance_pin = board.pins.d17;
    let cutoff_count = adc.blocking_read(&mut cutoff_pin, ADC_SAMPLE_TIME);
    let resonance_count = adc.blocking_read(&mut resonance_pin, ADC_SAMPLE_TIME);
    publish(cutoff_count, resonance_count);

    spawner.spawn(unwrap!(audio_loop(board.audio_peripherals, consumer)));
    spawner.spawn(unwrap!(status_led_loop(led)));
    spawner.spawn(unwrap!(ui_loop(button, producer)));

    let mut cutoff_tracker = PotTracker::start(cutoff_count);
    let mut resonance_tracker = PotTracker::start(resonance_count);
    loop {
        Timer::after_millis(POT_PERIOD_MS as u64).await;
        let cutoff_count = adc.blocking_read(&mut cutoff_pin, ADC_SAMPLE_TIME);
        let resonance_count = adc.blocking_read(&mut resonance_pin, ADC_SAMPLE_TIME);
        let cutoff_count = cutoff_tracker.push(cutoff_count, POT_PERIOD_MS);
        let resonance_count = resonance_tracker.push(resonance_count, POT_PERIOD_MS);
        publish(cutoff_count, resonance_count);
    }
}

fn publish(cutoff_count: u16, resonance_count: u16) {
    store_f32(&CUTOFF_BITS, cutoff_hz(cutoff_count));
    store_f32(&RESONANCE_BITS, resonance(resonance_count));
}

fn store_f32(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}

fn load_f32(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

async fn boot_flash(led: &mut Output<'static>) {
    for _ in 0..3 {
        led.set_high();
        Timer::after_millis(BOOT_FLASH_MS).await;
        led.set_low();
        Timer::after_millis(BOOT_FLASH_MS).await;
    }
}

#[embassy_executor::task]
async fn audio_loop(audio: AudioPeripherals<'static>, mut consumer: Consumer<'static, MixerEvent>) {
    let mixer = MIXER.init(Mixer::new(SAMPLE_RATE_HZ));
    mixer.apply(MixerEvent::ToInstance {
        instance: ENGINE_INSTANCE,
        event: InstanceEvent::SetVolume {
            amount: START_VOLUME,
        },
    });

    let idle = audio.prepare_interface(Default::default()).await;
    let Ok(mut interface) = idle.start_interface().await else {
        AUDIO_ERROR.store(true, Ordering::Relaxed);
        return;
    };
    if interface
        .start_callback(|_input, output| {
            while let Some(event) = consumer.dequeue() {
                mixer.apply(event);
            }
            apply_pots(mixer);
            for chunk in output.chunks_exact_mut(2) {
                let bits = f32_to_u24(mixer.next_sample());
                chunk[0] = bits;
                chunk[1] = bits;
            }
        })
        .await
        .is_err()
    {
        AUDIO_ERROR.store(true, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn status_led_loop(mut led: Output<'static>) {
    loop {
        if AUDIO_ERROR.load(Ordering::Relaxed) {
            led.set_high();
            Timer::after_millis(ERROR_BLINK_MS).await;
            led.set_low();
            Timer::after_millis(ERROR_BLINK_MS).await;
        } else {
            if GATE_LED_ON.load(Ordering::Relaxed) {
                led.set_high();
            } else {
                led.set_low();
            }
            Timer::after_millis(POLL_MS).await;
        }
    }
}

fn oled_flush(display: &mut OledDisplay) {
    let _ = display.flush();
}

fn oled_set_on(display: &mut OledDisplay, on: bool) {
    let _ = display.set_display_on(on);
}

fn apply_pots(mixer: &mut Mixer) {
    mixer.apply(MixerEvent::ToInstance {
        instance: ENGINE_INSTANCE,
        event: InstanceEvent::Engine(ControlEvent::SetCutoff {
            hz: load_f32(&CUTOFF_BITS),
        }),
    });
    mixer.apply(MixerEvent::ToInstance {
        instance: ENGINE_INSTANCE,
        event: InstanceEvent::Engine(ControlEvent::SetResonance {
            amount: load_f32(&RESONANCE_BITS),
        }),
    });
}

#[embassy_executor::task]
async fn ui_loop(button: Input<'static>, mut producer: Producer<'static, MixerEvent>) {
    let mut first_press = true;

    loop {
        wait_for_press(&button).await;
        if !first_press {
            apply_random(&mut producer);
        }
        first_press = false;
        wait_for_release(&button).await;
        play_until_press(&button, &mut producer).await;
    }
}

fn init_oled(i2c: OledI2c) -> Option<OledDisplay> {
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    match display.init() {
        Ok(()) => {
            info!("oled ready");
            let _ = display.clear(BinaryColor::Off);
            oled_flush(&mut display);
            Some(display)
        }
        Err(_) => {
            warn!("oled init failed; button and led still run");
            None
        }
    }
}

fn show_hello(display: &mut OledDisplay) {
    let _ = display.clear(BinaryColor::Off);
    let style = MonoTextStyleBuilder::new()
        .font(&FONT_5X8)
        .text_color(BinaryColor::On)
        .build();
    let _ = Text::with_baseline("Hello", Point::new(50, 28), style, Baseline::Top).draw(display);
    oled_flush(display);
}

async fn play_until_press(button: &Input<'_>, producer: &mut Producer<'static, MixerEvent>) {
    let last = NOTES.len() - 1;
    loop {
        for (index, note) in NOTES.iter().copied().enumerate() {
            GATE_LED_ON.store(true, Ordering::Relaxed);
            enqueue(
                producer,
                MixerEvent::MidiNoteOn {
                    channel: LISTEN_CHANNEL,
                    note,
                    velocity: NOTE_VELOCITY,
                },
            );
            if wait_or_press(button, GATE_MS).await {
                end_note(producer, note);
                return;
            }
            end_note(producer, note);
            let gap = if index == last { LOOP_GAP_MS } else { REST_MS };
            if wait_or_press(button, gap).await {
                return;
            }
        }
    }
}

fn end_note(producer: &mut Producer<'static, MixerEvent>, note: u8) {
    enqueue(
        producer,
        MixerEvent::MidiNoteOff {
            channel: LISTEN_CHANNEL,
            note,
        },
    );
    GATE_LED_ON.store(false, Ordering::Relaxed);
}

/// True when the button is pressed before `total_ms` elapses.
async fn wait_or_press(button: &Input<'_>, total_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(total_ms);
    loop {
        if button.is_low() {
            Timer::after_millis(DEBOUNCE_MS).await;
            if button.is_low() {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        Timer::after_millis(POLL_MS).await;
    }
}

fn apply_random(producer: &mut Producer<'static, MixerEvent>) {
    let mut rng = SmallRng::seed_from_u64(Instant::now().as_ticks());
    let (params, _) = random_patch(&mut rng);
    for event in patch_events(ENGINE_INSTANCE, &params, START_VOLUME).as_slice() {
        enqueue(producer, *event);
    }
}

fn enqueue(producer: &mut Producer<'static, MixerEvent>, event: MixerEvent) {
    if producer.enqueue(event).is_err() {
        warn!("event queue full");
    }
}

async fn wait_for_press(button: &Input<'_>) {
    loop {
        if button.is_low() {
            Timer::after_millis(DEBOUNCE_MS).await;
            if button.is_low() {
                return;
            }
        }
        Timer::after_millis(POLL_MS).await;
    }
}

async fn wait_for_release(button: &Input<'_>) {
    while button.is_low() {
        Timer::after_millis(POLL_MS).await;
    }
    Timer::after_millis(DEBOUNCE_MS).await;
}

fn f32_to_u24(x: f32) -> u32 {
    let x = x * 8_388_607.0;
    let x = x.clamp(-8_388_608.0, 8_388_607.0);
    (x as i32) as u32
}
