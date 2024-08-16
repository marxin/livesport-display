#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use core::str::from_utf8;

use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Config, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::OutputOpenDrain;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use rand::RngCore;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::Method;
use serde::Deserialize;
use serde_json_core::heapless::String;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use crate::tm1637::{get_digit_code, DIGITS, TM1637};

mod tm1637;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

const WIFI_NETWORK: &str = "marxin";
const DEFAULT_BRIGHTNESS_LEVEL: u8 = 3;

static SCORE_SIGNAL: Signal<CriticalSectionRawMutex, Option<(u64, u64)>> = Signal::new();
static TIME_SIGNAL: Signal<CriticalSectionRawMutex, GameTime> = Signal::new();

#[derive(Debug, Deserialize)]
enum GameTime {
    WillBePlayed(Option<(u64, u64)>),
    Played,
    BreakAfter(u64),
    Playing(u64),
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct GameResult {
    my_team: String<64>,
    my_team_score: u64,
    opponent_team: String<64>,
    opponent_team_score: u64,
    game_time: GameTime,
}

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[embassy_executor::task]
async fn update_score(mut score_display: TM1637<'static, 'static>) -> ! {
    score_display.turn_off().await;

    loop {
        let score = SCORE_SIGNAL.wait().await;
        if let Some((my_team_score, opponent_team_score)) = score {
            let mut digits = [
                Some((my_team_score / 10) % 10),
                Some(my_team_score % 10),
                Some((opponent_team_score / 10) % 10),
                Some(opponent_team_score % 10),
            ];
            // trim leading zero
            if digits[0] == Some(0) {
                digits[0] = None;
            }

            // trim trailing zero
            if digits[2] == Some(0) {
                digits[2] = digits[3];
                digits[3] = None;
            }

            let mut digit_codes = [0u8; 4];
            for i in 0..digits.len() {
                digit_codes[i] = get_digit_code(digits[i]);
            }

            score_display
                .display(digit_codes, true, DEFAULT_BRIGHTNESS_LEVEL)
                .await;
        } else {
            const EMPTY_DIGITS: [u8; 4] = [0u8; 4];

            score_display
                .display(EMPTY_DIGITS, true, DEFAULT_BRIGHTNESS_LEVEL)
                .await;
            Timer::after(Duration::from_secs(1)).await;
            score_display
                .display(EMPTY_DIGITS, false, DEFAULT_BRIGHTNESS_LEVEL)
                .await;
        }
    }
}

fn tuple_to_digits(tuple: (u64, u64)) -> [u8; 4] {
    [
        DIGITS[((tuple.0 / 10) % 10) as usize],
        DIGITS[(tuple.0 % 10) as usize],
        DIGITS[((tuple.1 / 10) % 10) as usize],
        DIGITS[(tuple.1 % 10) as usize],
    ]
}

#[embassy_executor::task]
async fn update_time(mut time_display: TM1637<'static, 'static>) -> ! {
    const COLON_BLINK_INTERVAL: Duration = Duration::from_millis(500);

    time_display.turn_off().await;

    loop {
        let game_time = TIME_SIGNAL.wait().await;
        match game_time {
            GameTime::Played => {
                time_display.turn_off().await;
            }
            GameTime::WillBePlayed(when) => {
                if let Some(when) = when {
                    time_display
                        .display(
                            tuple_to_digits((when.0, when.1)),
                            true,
                            DEFAULT_BRIGHTNESS_LEVEL,
                        )
                        .await;
                } else {
                    time_display.turn_off().await;
                }
            }
            GameTime::Playing(minute) => {
                let digits = tuple_to_digits((minute, 0));

                loop {
                    time_display
                        .display(digits, true, DEFAULT_BRIGHTNESS_LEVEL)
                        .await;
                    Timer::after(COLON_BLINK_INTERVAL).await;
                    time_display
                        .display(digits, false, DEFAULT_BRIGHTNESS_LEVEL)
                        .await;
                    Timer::after(COLON_BLINK_INTERVAL).await;
                    // get a new value
                    if TIME_SIGNAL.signaled() {
                        break;
                    }
                }
            }
            GameTime::BreakAfter(minute) => {
                time_display
                    .display(tuple_to_digits((minute, 0)), true, DEFAULT_BRIGHTNESS_LEVEL)
                    .await;
            }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Hello World!");

    let p = embassy_rp::init(Default::default());

    let score_display = TM1637::new(
        OutputOpenDrain::new(p.PIN_14, Level::Low),
        OutputOpenDrain::new(p.PIN_15, Level::Low),
    );
    unwrap!(spawner.spawn(update_score(score_display)));

    let time_display = TM1637::new(
        OutputOpenDrain::new(p.PIN_12, Level::Low),
        OutputOpenDrain::new(p.PIN_13, Level::Low),
    );
    unwrap!(spawner.spawn(update_time(time_display)));

    let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
    // To make flashing faster for development, you may want to flash the firmwares independently
    // at hardcoded addresses, instead of baking them into the program with `include_bytes!`:
    //     probe-rs download 43439A0.bin --binary-format bin --chip RP2040 --base-address 0x10100000
    //     probe-rs download 43439A0_clm.bin --binary-format bin --chip RP2040 --base-address 0x10140000
    // let fw = unsafe { core::slice::from_raw_parts(0x10100000 as *const u8, 230321) };
    // let clm = unsafe { core::slice::from_raw_parts(0x10140000 as *const u8, 4752) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs: Output = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());

    // Generate random seed
    let mut rng = RoscRng;
    let seed = rng.next_u64();

    // Init network stack
    static STACK: StaticCell<Stack<cyw43::NetDriver<'static>>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        net_device,
        config,
        RESOURCES.init(StackResources::<5>::new()),
        seed,
    ));

    unwrap!(spawner.spawn(net_task(stack)));

    info!("connecting to WiFi...");

    loop {
        //match control.join_open(WIFI_NETWORK).await { // for open networks
        match control
            .join_wpa2(WIFI_NETWORK, include_str!("../wifi_password.txt").trim())
            .await
        {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    control.gpio_set(0, true).await;

    // Wait for DHCP, not necessary when using static IP
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    info!("waiting for link up...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }
    info!("Link is up!");

    info!("waiting for stack to be up...");
    stack.wait_config_up().await;
    info!("Stack is up!");

    // And now we can use it!
    enum SleepState {
        FirstIteration,
        AfterSuccess,
        AfterFailure,
    }

    let mut sleep_state = SleepState::FirstIteration;

    loop {
        let mut rx_buffer = [0; 8192];
        let mut tls_read_buffer = [0; 16640];
        let mut tls_write_buffer = [0; 16640];

        let sleep_in_secs = match sleep_state {
            SleepState::FirstIteration => 0,
            SleepState::AfterSuccess => 5,
            SleepState::AfterFailure => 30,
        };

        Timer::after(Duration::from_secs(sleep_in_secs)).await;

        let client_state = TcpClientState::<1, 1024, 1024>::new();
        let tcp_client = TcpClient::new(stack, &client_state);
        let dns_client = DnsSocket::new(stack);
        let tls_config = TlsConfig::new(
            seed,
            &mut tls_read_buffer,
            &mut tls_write_buffer,
            TlsVerify::None,
        );

        let mut http_client = HttpClient::new_with_tls(&tcp_client, &dns_client, tls_config);
        let url = "https://marxin.eu/sparta.json";
        info!("connecting to {}", &url);

        let mut request = match http_client.request(Method::GET, url).await {
            Ok(req) => req,
            Err(e) => {
                error!("Failed to make HTTP request: {:?}", e);
                sleep_state = SleepState::AfterFailure;
                continue;
            }
        };

        let response = match request.send(&mut rx_buffer).await {
            Ok(resp) => resp,
            Err(_e) => {
                error!("Failed to send HTTP request");
                sleep_state = SleepState::AfterFailure;
                continue;
            }
        };

        let body = match from_utf8(response.body().read_to_end().await.unwrap()) {
            Ok(b) => b,
            Err(_e) => {
                error!("Failed to read response body");
                sleep_state = SleepState::AfterFailure;
                continue;
            }
        };
        info!("Response body: {:?}", &body);

        let bytes = body.as_bytes();
        match serde_json_core::de::from_slice::<GameResult>(bytes) {
            Ok((game_result, _used)) => {
                let score = if let GameTime::WillBePlayed(_) = game_result.game_time {
                    None
                } else {
                    Some((game_result.my_team_score, game_result.opponent_team_score))
                };
                SCORE_SIGNAL.signal(score);
                TIME_SIGNAL.signal(game_result.game_time);
            }
            Err(e) => {
                error!("Failed to parse response body: {}", e as u8);
                sleep_state = SleepState::AfterFailure;
                continue;
            }
        }

        sleep_state = SleepState::AfterSuccess;
    }
}
