use embassy_rp::gpio::OutputOpenDrain;
use embassy_time::Timer;

const DELAY_USECS: u64 = 100;

const ADDRESS_AUTO_INCREMENT_1_MODE: u8 = 0x40;
const ADDRESS_COMMAND_BITS: u8 = 0xc0;
const ADDRESS_COMM_3: u8 = 0x80;

const DISPLAY_CONTROL_BRIGHTNESS_MASK: u8 = 0x07;

// SEG_A 0b00000001
// SEG_B 0b00000010
// SEG_C 0b00000100
// SEG_D 0b00001000
// SEG_E 0b00010000
// SEG_F 0b00100000
// SEG_G 0b01000000
// SEG_DP 0b10000000

//
//      A
//     ---
//  F |   | B
//     -G-
//  E |   | C
//     ---
//      D
pub(crate) const DIGITS: [u8; 16] = [
    // XGFEDCBA
    0b00111111, // 0
    0b00000110, // 1
    0b01011011, // 2
    0b01001111, // 3
    0b01100110, // 4
    0b01101101, // 5
    0b01111101, // 6
    0b00000111, // 7
    0b01111111, // 8
    0b01101111, // 9
    0b01110111, // A
    0b01111100, // b
    0b00111001, // C
    0b01011110, // d
    0b01111001, // E
    0b01110001, // F
];

pub(crate) fn get_digit_code(digit: Option<u64>) -> u8 {
    match digit {
        None => 0x0,
        Some(digit) => DIGITS[digit as usize],
    }
}

pub(crate) struct TM1637<'clk, 'dio> {
    clk: OutputOpenDrain<'clk>,
    dio: OutputOpenDrain<'dio>,
}

impl<'clk, 'dio> TM1637<'clk, 'dio> {
    pub fn new(clk: OutputOpenDrain<'clk>, dio: OutputOpenDrain<'dio>) -> Self {
        Self { clk, dio }
    }

    async fn delay(&mut self) {
        Timer::after_micros(DELAY_USECS).await;
    }

    fn brightness(&self, level: u8, on: bool) -> u8 {
        (level & DISPLAY_CONTROL_BRIGHTNESS_MASK) | (if on { 0x08 } else { 0x00 })
    }

    pub async fn set_brightness(&mut self, level: u8, on: bool) {
        self.start().await;
        let brightness = self.brightness(level, on);
        self.write_cmd(ADDRESS_COMM_3 + (brightness & 0x0f)).await;
        self.stop().await;
    }

    pub async fn turn_off(&mut self) {
        self.set_brightness(0, false).await;
    }

    async fn send_bit_and_delay(&mut self, bit: bool) {
        self.clk.set_low();
        self.delay().await;
        if bit {
            self.dio.set_high();
        } else {
            self.dio.set_low();
        }
        self.delay().await;
        self.clk.set_high();
        self.delay().await;
    }

    pub async fn write_byte(&mut self, data: u8) {
        for i in 0..8 {
            self.send_bit_and_delay((data >> i) & 0x01 != 0).await;
        }
        self.clk.set_low();
        self.delay().await;
        self.dio.set_high();
        self.delay().await;
        self.clk.set_high();
        self.delay().await;
        self.dio.wait_for_low().await;
    }

    pub async fn start(&mut self) {
        self.clk.set_high();
        self.dio.set_high();
        self.delay().await;
        self.dio.set_low();
        self.delay().await;
        self.clk.set_low();
        self.delay().await;
    }

    pub async fn stop(&mut self) {
        self.clk.set_low();
        self.delay().await;
        self.dio.set_low();
        self.delay().await;
        self.clk.set_high();
        self.delay().await;
        self.dio.set_high();
        self.delay().await;
    }

    pub async fn write_cmd(&mut self, cmd: u8) {
        self.start().await;
        self.write_byte(cmd).await;
        self.stop().await;
    }

    pub async fn write_data(&mut self, addr: u8, data: u8) {
        self.start().await;
        self.write_byte(addr).await;
        self.write_byte(data).await;
        self.stop().await;
    }

    pub async fn display(&mut self, data: [u8; 4], show_colon: bool, brightness: u8) {
        self.write_cmd(ADDRESS_AUTO_INCREMENT_1_MODE).await;
        self.start().await;
        let mut address = ADDRESS_COMMAND_BITS;
        for (index, mut data_item) in data.into_iter().enumerate() {
            if index == 1 && show_colon {
                data_item |= 0b10000000;
            }
            self.write_data(address, data_item).await;
            address += 1;
        }
        self.set_brightness(brightness, true).await;
    }
}
