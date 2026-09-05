#![no_std]
#![no_main]

//! Breadboard GPIO check. USB powers the Seed.
//!
//! Wiring (see `daisy-seed-3-pinout-diagram.png` at the repo root):
//! - D24 = physical pin 31. LED output; high turns the LED on. Series resistor 330 Ω to 1 kΩ required.
//! - D15 = physical pin 22. Button input with internal pull-up. Press shorts this pin to GND.
//! - GND = physical pin 40. Shared ground for the LED cathode and the button.
//! Pin 21 next to D15 is 3V3 analog power. Do not put the button there.

use daisy_embassy::hal::gpio::{Input, Level, Output, Pull, Speed};
use daisy_embassy::{hal, new_daisy_board};
use embassy_executor::Spawner;
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

const FLASH_MS: u64 = 100;
const BETWEEN_FLASHES_MS: u64 = 100;
const DEBOUNCE_MS: u64 = 30;
const POLL_MS: u64 = 10;
const FLASH_COUNT: u8 = 3;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let peripherals = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(peripherals);
    let mut user_led = board.user_led;

    user_led.on();
    Timer::after_millis(FLASH_MS).await;
    user_led.off();

    let mut led = Output::new(board.pins.d24, Level::Low, Speed::Low);
    let button = Input::new(board.pins.d15, Pull::Up);

    loop {
        if button.is_low() {
            Timer::after_millis(DEBOUNCE_MS).await;
            if button.is_low() {
                for _ in 0..FLASH_COUNT {
                    led.set_high();
                    Timer::after_millis(FLASH_MS).await;
                    led.set_low();
                    Timer::after_millis(BETWEEN_FLASHES_MS).await;
                }
                while button.is_low() {
                    Timer::after_millis(POLL_MS).await;
                }
                Timer::after_millis(DEBOUNCE_MS).await;
            }
        }
        Timer::after_millis(POLL_MS).await;
    }
}
