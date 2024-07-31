#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::gpio;
use embassy_time::Timer;
use gpio::{Level, OutputOpenDrain};
use {defmt_rtt as _, panic_probe as _};

use crate::tm1637::{DIGITS, TM1637};

mod tm1637;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let clock_pin = OutputOpenDrain::new(p.PIN_14, Level::Low);
    let dio_pin = OutputOpenDrain::new(p.PIN_15, Level::Low);
    let mut tm = TM1637::new(clock_pin, dio_pin);

    let mut number = 0;
    loop {
        let digits: [u8; 4] = [DIGITS[1], DIGITS[2], DIGITS[3], DIGITS[number]];

        tm.display(digits, true, 3, true).await;
        Timer::after_millis(500).await;
        tm.display(digits, false, 3, true).await;
        Timer::after_millis(500).await;

        number = (number + 1) % 10;
    }
}
