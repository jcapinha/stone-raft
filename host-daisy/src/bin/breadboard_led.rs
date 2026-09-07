#![no_std]
#![no_main]

//! Breadboard synth check. USB powers the Seed.
//!
//! Wiring (see `daisy-seed-3-pinout-diagram.png` at the repo root):
//! - D11 = physical pin 12. OLED I2C SCL (module SCK).
//! - D12 = physical pin 13. OLED I2C SDA.
//! - D15 = physical pin 22. Button input with internal pull-up. Press shorts this pin to GND.
//! - D24 = physical pin 31. LED output; high turns the LED on during each 1 s gate. Series resistor 330 Ω to 1 kΩ required
//! - Audio Out 1 = physical pin 18. Line-level mono (copy to both codec channels).
//! - AGND = physical pin 20. Audio ground for the jack sleeve.
//! - 3V3 digital = physical pin 38. OLED VDD. Do not use analog 3V3 on pin 21.
//! - GND = physical pin 40. Shared ground for OLED, LED cathode, and button.
//!
//! Boot shows `Hello` for 3 s, then the OLED sleeps. First button press plays C4-E4-G4
//! with the default saw patch. Later presses run `random`, then the same arpeggio.
//! The OLED is a rolling scope while notes sound, then a condensed patch card.

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use daisy_embassy::audio::{Idle, Interface};
use daisy_embassy::hal::gpio::{Input, Level, Output, Pull, Speed};
use daisy_embassy::hal::i2c::I2c;
use daisy_embassy::hal::time::Hertz;
use daisy_embassy::hal::{bind_interrupts, dma, i2c, peripherals};
use daisy_embassy::{hal, new_daisy_board};
use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::{FONT_5X8, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle};
use embedded_graphics::text::{Baseline, Text};
use engine::{
    AssignableDest, EngineParams, InstanceEvent, Mixer, MixerEvent, patch_events, random_patch,
};
use heapless::String;
use heapless::spsc::{Producer, Queue};
use rand::SeedableRng;
use rand::rngs::SmallRng;
use ssd1306::mode::{BufferedGraphicsModeAsync, DisplayConfigAsync};
use ssd1306::prelude::{DisplayRotation, DisplaySize128x64};
use ssd1306::{I2CDisplayInterface, Ssd1306Async};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct I2cIrqs {
    I2C1_EV => i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C1_ER => i2c::ErrorInterruptHandler<peripherals::I2C1>;
    DMA1_STREAM2 => dma::InterruptHandler<peripherals::DMA1_CH2>;
    DMA1_STREAM3 => dma::InterruptHandler<peripherals::DMA1_CH3>;
});

const SAMPLE_RATE_HZ: f32 = 48_000.0;
const START_VOLUME: f32 = 0.4;
const NOTE_VELOCITY: u8 = 100;
const LISTEN_CHANNEL: u8 = 1;
const ENGINE_INSTANCE: u8 = 1;
const NOTES: [u8; 3] = [60, 64, 67];
const GATE_MS: u64 = 1_000;
const REST_MS: u64 = 2_000;
const RELEASE_MS: u64 = 300;
const HELLO_MS: u64 = 3_000;
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 10;
const SCOPE_FRAME_MS: u64 = 50;
const SCOPE_LEN: usize = 128;
const SCOPE_DOWNSAMPLE: u8 = 8;
const EVENT_QUEUE_CAP: usize = 64;

type OledI2c = I2c<'static, daisy_embassy::hal::mode::Async, i2c::mode::Master>;
type OledDisplay = Ssd1306Async<
    ssd1306::prelude::I2CInterface<OledI2c>,
    DisplaySize128x64,
    BufferedGraphicsModeAsync<DisplaySize128x64>,
>;

static MIXER: StaticCell<Mixer> = StaticCell::new();
static EVENT_QUEUE: StaticCell<Queue<MixerEvent, EVENT_QUEUE_CAP>> = StaticCell::new();

struct ScopeBuffer {
    samples: [AtomicU32; SCOPE_LEN],
    write: AtomicUsize,
}

impl ScopeBuffer {
    const fn new() -> Self {
        const ZERO: AtomicU32 = AtomicU32::new(0);
        Self {
            samples: [ZERO; SCOPE_LEN],
            write: AtomicUsize::new(0),
        }
    }

    fn push(&self, sample: f32) {
        let index = self.write.load(Ordering::Relaxed);
        self.samples[index].store(sample.to_bits(), Ordering::Relaxed);
        self.write.store((index + 1) % SCOPE_LEN, Ordering::Release);
    }

    fn snapshot(&self) -> [f32; SCOPE_LEN] {
        let write = self.write.load(Ordering::Acquire);
        let mut out = [0.0; SCOPE_LEN];
        let mut i = 0;
        while i < SCOPE_LEN {
            let index = (write + i) % SCOPE_LEN;
            out[i] = f32::from_bits(self.samples[index].load(Ordering::Relaxed));
            i += 1;
        }
        out
    }
}

static SCOPE: ScopeBuffer = ScopeBuffer::new();

struct PatchShadow {
    params: EngineParams,
    volume: f32,
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(p);

    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = Hertz::khz(400);
    let i2c = I2c::new(
        p.I2C1,
        board.pins.d11,
        board.pins.d12,
        p.DMA1_CH2,
        p.DMA1_CH3,
        I2cIrqs,
        i2c_config,
    );

    let interface = board
        .audio_peripherals
        .prepare_interface(Default::default())
        .await;

    let queue = EVENT_QUEUE.init(Queue::new());
    let (producer, consumer) = queue.split();

    let button = Input::new(board.pins.d15, Pull::Up);
    let led = Output::new(board.pins.d24, Level::Low, Speed::Low);

    join(
        audio_loop(interface, consumer),
        ui_loop(i2c, button, led, producer),
    )
    .await;
}

async fn audio_loop(
    interface: Interface<'static, Idle>,
    mut consumer: heapless::spsc::Consumer<'static, MixerEvent>,
) {
    let mixer = MIXER.init(Mixer::new(SAMPLE_RATE_HZ));
    mixer.apply(MixerEvent::ToInstance {
        instance: ENGINE_INSTANCE,
        event: InstanceEvent::SetVolume {
            amount: START_VOLUME,
        },
    });

    let mut interface = unwrap!(interface.start_interface().await);
    let mut downsample = 0u8;
    unwrap!(
        interface
            .start_callback(move |_input, output| {
                while let Some(event) = consumer.dequeue() {
                    mixer.apply(event);
                }
                for chunk in output.chunks_exact_mut(2) {
                    let sample = mixer.next_sample();
                    let bits = f32_to_u24(sample);
                    chunk[0] = bits;
                    chunk[1] = bits;
                    downsample = downsample.wrapping_add(1);
                    if downsample >= SCOPE_DOWNSAMPLE {
                        downsample = 0;
                        SCOPE.push(sample);
                    }
                }
            })
            .await
    );
}

async fn ui_loop(
    i2c: OledI2c,
    button: Input<'static>,
    mut led: Output<'static>,
    mut producer: Producer<'static, MixerEvent>,
) {
    let mut display = match init_oled(i2c).await {
        Some(display) => {
            info!("oled ready");
            Some(display)
        }
        None => {
            warn!("oled init failed; audio and button still run");
            None
        }
    };

    if let Some(display) = display.as_mut() {
        show_hello(display).await;
        Timer::after_millis(HELLO_MS).await;
        let _ = display.clear(BinaryColor::Off);
        let _ = display.flush().await;
        let _ = display.set_display_on(false).await;
    } else {
        Timer::after_millis(HELLO_MS).await;
    }

    let mut shadow = PatchShadow {
        params: EngineParams::default(),
        volume: START_VOLUME,
    };
    let mut first_press = true;

    loop {
        wait_for_press(&button).await;
        if !first_press {
            apply_random(&mut producer, &mut shadow);
        }
        first_press = false;

        if let Some(display) = display.as_mut() {
            let _ = display.set_display_on(true).await;
        }
        play_arpeggio(&mut producer, &mut led, display.as_mut()).await;
        Timer::after_millis(RELEASE_MS).await;
        if let Some(display) = display.as_mut() {
            show_patch_card(display, &shadow).await;
        }
        wait_for_release(&button).await;
    }
}

async fn init_oled(i2c: OledI2c) -> Option<OledDisplay> {
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306Async::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().await.ok()?;
    let _ = display.clear(BinaryColor::Off);
    let _ = display.flush().await;
    Some(display)
}

async fn show_hello(display: &mut OledDisplay) {
    let _ = display.clear(BinaryColor::Off);
    let style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(BinaryColor::On)
        .build();
    let _ = Text::with_baseline("Hello", Point::new(36, 22), style, Baseline::Top).draw(display);
    let _ = display.flush().await;
}

async fn play_arpeggio(
    producer: &mut Producer<'static, MixerEvent>,
    led: &mut Output<'static>,
    mut display: Option<&mut OledDisplay>,
) {
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
        scope_for(GATE_MS, display.as_deref_mut()).await;
        enqueue(
            producer,
            MixerEvent::MidiNoteOff {
                channel: LISTEN_CHANNEL,
                note,
            },
        );
        led.set_low();
        if index != last {
            scope_for(REST_MS, display.as_deref_mut()).await;
        }
    }
}

async fn scope_for(ms: u64, mut display: Option<&mut OledDisplay>) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if let Some(display) = display.as_deref_mut() {
            draw_scope(display).await;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let frame = Duration::from_millis(SCOPE_FRAME_MS);
        Timer::after(core::cmp::min(remaining, frame)).await;
    }
}

async fn draw_scope(display: &mut OledDisplay) {
    let samples = SCOPE.snapshot();
    let _ = display.clear(BinaryColor::Off);
    let style = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let mut x = 0;
    while x < SCOPE_LEN - 1 {
        let y0 = sample_to_y(samples[x]);
        let y1 = sample_to_y(samples[x + 1]);
        let _ = Line::new(Point::new(x as i32, y0), Point::new((x + 1) as i32, y1))
            .into_styled(style)
            .draw(display);
        x += 1;
    }
    let _ = display.flush().await;
}

async fn show_patch_card(display: &mut OledDisplay, shadow: &PatchShadow) {
    let _ = display.clear(BinaryColor::Off);
    let style = MonoTextStyleBuilder::new()
        .font(&FONT_5X8)
        .text_color(BinaryColor::On)
        .build();
    let lines = patch_card_lines(shadow);
    let mut y = 0;
    for line in lines.iter() {
        let _ = Text::with_baseline(line.as_str(), Point::new(0, y), style, Baseline::Top)
            .draw(display);
        y += 8;
    }
    let _ = display.flush().await;
}

fn patch_card_lines(shadow: &PatchShadow) -> [String<24>; 8] {
    let p = &shadow.params;
    let mut lines = [const { String::new() }; 8];
    let _ = write!(lines[0], "v{:.2} saw{:.2}", shadow.volume, p.saw_vol);
    let _ = write!(lines[1], "sq{:.2} tri{:.2}", p.square_vol, p.triangle_vol);
    let _ = write!(lines[2], "sin{:.2} sub{:.2}", p.sine_vol, p.sub_vol);
    let _ = write!(lines[3], "pw{:.2} cut{:.0}", p.pulse_width, p.cutoff_hz);
    let _ = write!(
        lines[4],
        "res{:.2} a{:.0}/{:.0}",
        p.resonance, p.amp_env.attack_ms, p.amp_env.decay_ms
    );
    let _ = write!(
        lines[5],
        "s{:.2} r{:.0}",
        p.amp_env.sustain, p.amp_env.release_ms
    );
    let _ = write!(
        lines[6],
        "f{:.2} as {}",
        p.filter_env_amount,
        dest_name(p.assignable_dest)
    );
    let _ = write!(
        lines[7],
        "L1 {} L2 {}",
        dest_name(p.lfos[0].dest),
        dest_name(p.lfos[1].dest)
    );
    lines
}

fn dest_name(dest: AssignableDest) -> &'static str {
    match dest {
        AssignableDest::Off => "off",
        AssignableDest::Resonance => "res",
        AssignableDest::Pitch => "pitch",
        AssignableDest::Cutoff => "cut",
        AssignableDest::PulseWidth => "pw",
        AssignableDest::Amp => "amp",
    }
}

fn apply_random(producer: &mut Producer<'static, MixerEvent>, shadow: &mut PatchShadow) {
    let mut rng = SmallRng::seed_from_u64(Instant::now().as_ticks());
    let (params, volume) = random_patch(&mut rng);
    shadow.params = params;
    shadow.volume = volume;
    for event in patch_events(ENGINE_INSTANCE, &params, volume).as_slice() {
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

fn sample_to_y(sample: f32) -> i32 {
    let y = 32.0 - sample * 28.0;
    y.clamp(0.0, 63.0) as i32
}

fn f32_to_u24(x: f32) -> u32 {
    let x = x * 8_388_607.0;
    let x = x.clamp(-8_388_608.0, 8_388_607.0);
    (x as i32) as u32
}
