#![no_std]
#![no_main]

use daisy_embassy::{hal, new_daisy_board};
use embassy_executor::Spawner;
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

const FLASH_MS: u64 = 100;
const BETWEEN_FLASHES_MS: u64 = 100;
const BETWEEN_PAIRS_MS: u64 = 1_700;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let peripherals = hal::init(daisy_embassy::default_rcc());
    let board = new_daisy_board!(peripherals);
    let mut led = board.user_led;

    loop {
        led.on();
        Timer::after_millis(FLASH_MS).await;
        led.off();
        Timer::after_millis(BETWEEN_FLASHES_MS).await;
        led.on();
        Timer::after_millis(FLASH_MS).await;
        led.off();
        Timer::after_millis(BETWEEN_PAIRS_MS).await;
    }
}
