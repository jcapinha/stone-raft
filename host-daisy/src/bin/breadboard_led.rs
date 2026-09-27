#![no_std]
#![no_main]

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
//! - 3V3 digital = physical pin 38. OLED VDD. Do not use analog 3V3 on pin 21.
//! - GND = physical pin 40. Shared ground for OLED, LED cathode, and button.
//!
//! Boot flashes the breadboard LED three times. OLED uses blocking I2C at 400 kHz
//! with a 200 ms timeout so a missing screen cannot freeze the button. Hello is
//! drawn, held for 3 s, then the screen sleeps, all before audio starts. After
//! that the Seed does not talk to the OLED. Volume is 1.0 on first press and
//! after `random`. Each press plays C4-E4-G4 with the LED on during each 1 s
//! gate. Later presses run `random`, then the same arpeggio. Audio uses the
//! daisy-embassy Seed 3 callback (TX and RX paced to the codec). An SAI error
//! stops audio and changes the LED to a continuous rapid blink until reset.

use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m::peripheral::Peripherals;
use daisy_embassy::audio::AudioPeripherals;
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
use engine::{InstanceEvent, Mixer, MixerEvent, patch_events, random_patch};
use heapless::spsc::{Consumer, Producer, Queue};
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
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

    spawner.spawn(unwrap!(audio_loop(board.audio_peripherals, consumer)));
    spawner.spawn(unwrap!(status_led_loop(led)));

    ui_loop(button, producer).await;
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

async fn ui_loop(button: Input<'static>, mut producer: Producer<'static, MixerEvent>) {
    let mut first_press = true;

    loop {
        wait_for_press(&button).await;
        if !first_press {
            apply_random(&mut producer);
        }
        first_press = false;
        play_arpeggio(&mut producer).await;
        wait_for_release(&button).await;
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

async fn play_arpeggio(producer: &mut Producer<'static, MixerEvent>) {
    let last = NOTES.len() - 1;
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
        Timer::after_millis(GATE_MS).await;
        enqueue(
            producer,
            MixerEvent::MidiNoteOff {
                channel: LISTEN_CHANNEL,
                note,
            },
        );
        GATE_LED_ON.store(false, Ordering::Relaxed);
        if index != last {
            Timer::after_millis(REST_MS).await;
        }
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
