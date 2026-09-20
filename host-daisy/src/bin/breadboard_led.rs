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
//! gate. Later presses run `random`, then the same arpeggio. Audio runs on an
//! interrupt executor. SPI4 is masked during blocking OLED transfers so I2C is
//! not cut off mid-frame; those transfers only run before the codec starts.
//! Recoverable DMA overruns yield, then refill.

use core::slice;

use daisy_embassy::audio::{
    AudioIrqs, AudioPeripherals, DMA_BUFFER_LENGTH, Fs, HALF_DMA_BUFFER_LENGTH,
};
use daisy_embassy::hal::gpio::{Input, Level, Output, Pull, Speed};
use daisy_embassy::hal::i2c::I2c;
use daisy_embassy::hal::interrupt::{self, InterruptExt, Priority};
use daisy_embassy::hal::sai::{self, Sai};
use daisy_embassy::hal::time::Hertz;
use daisy_embassy::hal::{i2c, peripherals};
use daisy_embassy::{hal, new_daisy_board};
use defmt::{info, unwrap, warn};
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::FONT_5X8;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use engine::{InstanceEvent, Mixer, MixerEvent, patch_events, random_patch};
use grounded::uninit::GroundedArrayCell;
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
static AUDIO_EXECUTOR: InterruptExecutor = InterruptExecutor::new();
#[unsafe(link_section = ".sram1_bss")]
static AUDIO_TX_BUFFER: GroundedArrayCell<u32, DMA_BUFFER_LENGTH> = GroundedArrayCell::uninit();

#[cortex_m_rt::interrupt]
unsafe fn SPI4() {
    unsafe {
        AUDIO_EXECUTOR.on_interrupt();
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(p);

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
        Timer::after_millis(HELLO_MS).await;
        let _ = display.clear(BinaryColor::Off);
        oled_flush(display);
        oled_set_on(display, false);
    }
    drop(display);

    let queue = EVENT_QUEUE.init(Queue::new());
    let (producer, consumer) = queue.split();

    interrupt::SPI4.set_priority(Priority::P1);
    let audio_spawner = AUDIO_EXECUTOR.start(interrupt::SPI4);
    audio_spawner.spawn(unwrap!(audio_loop(board.audio_peripherals, consumer)));

    ui_loop(button, led, producer).await;
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

    let mut output = prepare_audio_output(audio);
    let mut write_buffer = [0; HALF_DMA_BUFFER_LENGTH];
    Timer::after_millis(2).await;
    if output.write(&write_buffer).await.is_err() {
        warn!("audio start failed");
        loop {
            Timer::after_millis(1_000).await;
        }
    }

    loop {
        while let Some(event) = consumer.dequeue() {
            mixer.apply(event);
        }
        for chunk in write_buffer.chunks_exact_mut(2) {
            let sample = mixer.next_sample();
            let bits = f32_to_u24(sample);
            chunk[0] = bits;
            chunk[1] = bits;
        }
        if output.write(&write_buffer).await.is_err() {
            warn!("audio output overrun; ring reset");
            Timer::after_millis(1).await;
        }
    }
}

fn prepare_audio_output(audio: AudioPeripherals<'static>) -> Sai<'static, peripherals::SAI1, u32> {
    let AudioPeripherals {
        codec_pins,
        sai1,
        dma1_ch0,
        ..
    } = audio;
    // SAFETY: this is the single initialization of the static DMA buffer, and
    // SAI owns the returned slice for the rest of the firmware's lifetime.
    let tx_buffer = unsafe {
        AUDIO_TX_BUFFER.initialize_all_copied(0);
        let (pointer, length) = AUDIO_TX_BUFFER.get_ptr_len();
        slice::from_raw_parts_mut(pointer, length)
    };
    let (tx, _rx) = sai::split_subblocks(sai1);

    let mut config = sai::Config::default();
    config.mode = sai::Mode::Master;
    config.tx_rx = sai::TxRx::Transmitter;
    config.sync_output = true;
    config.clock_strobe = sai::ClockStrobe::Falling;
    config.master_clock_divider = Fs::Fs48000.into_clock_divider();
    config.stereo_mono = sai::StereoMono::Stereo;
    config.data_size = sai::DataSize::Data32;
    config.bit_order = sai::BitOrder::MsbFirst;
    config.frame_sync_polarity = sai::FrameSyncPolarity::ActiveHigh;
    config.frame_sync_offset = sai::FrameSyncOffset::OnFirstBit;
    config.frame_length = 64;
    config.frame_sync_active_level_length = sai::word::U7(32);
    config.fifo_threshold = sai::FifoThreshold::Quarter;

    Sai::new_asynchronous_with_mclk(
        tx,
        codec_pins.SCK_A,
        codec_pins.SD_A,
        codec_pins.FS_A,
        codec_pins.MCLK_A,
        dma1_ch0,
        tx_buffer,
        AudioIrqs,
        config,
    )
}

fn with_oled_bus_masked<R>(f: impl FnOnce() -> R) -> R {
    let audio_was_running = interrupt::SPI4.is_enabled();
    if audio_was_running {
        interrupt::SPI4.disable();
    }
    let result = f();
    if audio_was_running {
        // SAFETY: SPI4 was already enabled for AUDIO_EXECUTOR. Masking it only
        // around blocking OLED transfers keeps I2C from being cut off mid-frame.
        unsafe {
            interrupt::SPI4.enable();
        }
    }
    result
}

fn oled_flush(display: &mut OledDisplay) {
    with_oled_bus_masked(|| {
        let _ = display.flush();
    });
}

fn oled_set_on(display: &mut OledDisplay, on: bool) {
    with_oled_bus_masked(|| {
        let _ = display.set_display_on(on);
    });
}

async fn ui_loop(
    button: Input<'static>,
    mut led: Output<'static>,
    mut producer: Producer<'static, MixerEvent>,
) {
    let mut first_press = true;

    loop {
        wait_for_press(&button).await;
        if !first_press {
            apply_random(&mut producer);
        }
        first_press = false;
        play_arpeggio(&mut producer, &mut led).await;
        wait_for_release(&button).await;
    }
}

fn init_oled(i2c: OledI2c) -> Option<OledDisplay> {
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    match with_oled_bus_masked(|| display.init()) {
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

async fn play_arpeggio(producer: &mut Producer<'static, MixerEvent>, led: &mut Output<'static>) {
    let last = NOTES.len() - 1;
    for (index, note) in NOTES.iter().copied().enumerate() {
        led.set_high();
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
        led.set_low();
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
