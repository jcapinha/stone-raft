#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

//! Bench listen path. USB powers the Seed and, while this firmware runs, is also USB MIDI.
//!
//! Wiring (see `host-daisy/docs/filter-pots.md`):
//! - Cutoff pot wiper, through 1 kΩ: A1 / D16, physical pin 23 (`PA3`).
//! - Resonance pot wiper, through 1 kΩ: A2 / D17, physical pin 24 (`PB1`).
//! - Both pot ends: 3V3 analog pin 21 and AGND pin 20.
//! - 100 nF from each ADC pin to AGND. No software glide.
//! - Audio Out 1 and 2, pins 18 and 19, plus AGND pin 20. Same mono mix on both.
//!
//! D15 and D24 stay unused. No OLED. To flash again, hold BOOT and reset so DFU returns.
//! USB MIDI exists only while this binary is running. Product string is `stone-raft`.

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::Peripherals;
use daisy_embassy::audio::AudioPeripherals;
use daisy_embassy::hal::adc::{Adc, SampleTime};
use daisy_embassy::hal::usb::{Config as UsbDriverConfig, Driver};
use daisy_embassy::hal::{bind_interrupts, peripherals, usb};
use daisy_embassy::usb::UsbPeripherals;
use daisy_embassy::{hal, new_daisy_board};
use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_time::Timer;
use embassy_usb::Builder;
use embassy_usb::class::midi::MidiClass;
use embassy_usb::driver::EndpointError;
use engine::{ControlEvent, InstanceEvent, Mixer, MixerEvent};
use heapless::spsc::{Consumer, Producer, Queue};
use host_daisy::{PotTracker, cutoff_hz, resonance};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const SAMPLE_RATE_HZ: f32 = 48_000.0;
const START_VOLUME: f32 = 1.0;
const ENGINE_INSTANCE: u8 = 1;
const EVENT_QUEUE_CAP: usize = 64;
const POT_PERIOD_MS: u32 = 1;
const ADC_SAMPLE_TIME: SampleTime = SampleTime::CYCLES810_5;

bind_interrupts!(struct Irqs {
    OTG_FS => usb::InterruptHandler<peripherals::USB_OTG_FS>;
});

static MIXER: StaticCell<Mixer> = StaticCell::new();
static EVENT_QUEUE: StaticCell<Queue<MixerEvent, EVENT_QUEUE_CAP>> = StaticCell::new();
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

    // 16-bit is the H7 ADC reset resolution, matching `ADC_MAX_COUNT`.
    let mut adc = Adc::new(p.ADC1);
    let mut cutoff_pin = board.pins.d16;
    let mut resonance_pin = board.pins.d17;
    let cutoff_count = adc.blocking_read(&mut cutoff_pin, ADC_SAMPLE_TIME);
    let resonance_count = adc.blocking_read(&mut resonance_pin, ADC_SAMPLE_TIME);
    publish(cutoff_count, resonance_count);
    info!(
        "initial cutoff count {}, resonance count {}",
        cutoff_count, resonance_count
    );

    let queue = EVENT_QUEUE.init(Queue::new());
    let (producer, consumer) = queue.split();

    spawner.spawn(unwrap!(audio_loop(board.audio_peripherals, consumer)));
    spawner.spawn(unwrap!(usb_midi(board.usb_peripherals, producer)));

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
        warn!("audio interface failed to start");
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
        warn!("audio interface stopped");
    }
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
async fn usb_midi(
    peripherals: UsbPeripherals<'static>,
    mut producer: Producer<'static, MixerEvent>,
) {
    let mut config = UsbDriverConfig::default();
    config.vbus_detection = false;

    static EP_OUT_BUFFER: StaticCell<[u8; 256]> = StaticCell::new();
    let ep_out_buffer = EP_OUT_BUFFER.init([0; 256]);
    let driver = Driver::new_fs(
        peripherals.usb_otg_fs,
        Irqs,
        peripherals.pins.DP,
        peripherals.pins.DN,
        ep_out_buffer,
        config,
    );

    let mut config = embassy_usb::Config::new(0xdead, 0xc0de);
    config.manufacturer = Some("stone-raft");
    config.product = Some("stone-raft");
    config.serial_number = Some("bench");
    // Same composite setup as the daisy-embassy USB MIDI example, so Windows will enumerate it.
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [],
        &mut control_buf,
    );
    let mut class = MidiClass::new(&mut builder, 1, 1, 64);
    let mut usb = builder.build();

    let usb_fut = usb.run();
    let midi_fut = async {
        loop {
            class.wait_connection().await;
            info!("usb midi connected");
            read_notes(&mut class, &mut producer).await;
            info!("usb midi disconnected");
        }
    };
    join(usb_fut, midi_fut).await;
}

async fn read_notes<'d, T: daisy_embassy::hal::usb::Instance + 'd>(
    class: &mut MidiClass<'d, Driver<'d, T>>,
    producer: &mut Producer<'static, MixerEvent>,
) {
    let mut buf = [0; 64];
    loop {
        match class.read_packet(&mut buf).await {
            Ok(n) => enqueue_usb_midi(&buf[..n], producer),
            Err(EndpointError::Disabled) => return,
            Err(EndpointError::BufferOverflow) => warn!("usb midi packet overflow"),
        }
    }
}

/// USB MIDI packets are 4 bytes. Cable number is the high nibble. Note on is code 0x9, note off is 0x8.
fn enqueue_usb_midi(packet: &[u8], producer: &mut Producer<'static, MixerEvent>) {
    let mut offset = 0;
    while offset + 4 <= packet.len() {
        let code = packet[offset] & 0x0F;
        if code == 0x08 || code == 0x09 {
            if let Some(event) = MixerEvent::from_midi_bytes(&packet[offset + 1..offset + 4]) {
                if producer.enqueue(event).is_err() {
                    warn!("event queue full");
                }
            }
        }
        offset += 4;
    }
}

fn f32_to_u24(x: f32) -> u32 {
    let x = x * 8_388_607.0;
    let x = x.clamp(-8_388_608.0, 8_388_607.0);
    (x as i32) as u32
}
