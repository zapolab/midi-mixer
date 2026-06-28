#![no_std]
#![no_main]

use core::fmt::Write;
use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::{
    adc::{Adc, Channel, Config as AdcConfig, InterruptHandler as AdcInterruptHandler},
    bind_interrupts,
    gpio::{self, Pull},
    i2c::{Async, Config as I2cConfig, I2c, InterruptHandler as I2cInterruptHandler},
    peripherals::I2C0,
};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    text::Text,
};
use gpio::{Level, Output};
use heapless::String;
use ssd1306::{
    I2CDisplayInterface, Ssd1306, prelude::*, rotation::DisplayRotation, size::DisplaySize128x64,
};
use {defmt_rtt as _, panic_probe as _};

/// Struct for potentiometer read values
#[derive(Clone, Copy)] // needed for not losing ownership when sending into channel
struct PotReadings {
    filtered_pot0: u16,
    filtered_pot1: u16,
    percentage_pot0: u8,
    percentage_pot1: u8,
    midi_range_pot0: u8,
    midi_range_pot1: u8,
}

/// Display channel, capacity 2
static DISPLAY_CHANNEL: embassy_sync::channel::Channel<ThreadModeRawMutex, PotReadings, 2> =
    embassy_sync::channel::Channel::new();

bind_interrupts!(
    struct Irqs {
        ADC_IRQ_FIFO => AdcInterruptHandler;
        I2C0_IRQ => I2cInterruptHandler<I2C0>;
    }
);

/// Read 16 values and return the average
async fn read_adc_averaged(adc: &mut Adc<'_, embassy_rp::adc::Async>, ch: &mut Channel<'_>) -> u16 {
    let mut sum: u32 = 0;
    for _ in 0..16 {
        let val = adc.read(ch).await.unwrap();
        sum += val as u32;
        Timer::after_micros(100).await;
    }
    (sum / 16 as u32) as u16
}

/// From ADC filtered value to percentage
fn filtered_to_percent(filtered: u16) -> u8 {
    ((filtered as u32 * 100) / 4095) as u8
}

/// Converts ADC filtered to 0–127 MIDI range
fn filtered_to_midi_range(raw: u16) -> u8 {
    ((raw as u32 * 127) / 4095).min(127) as u8
}

/// Async task displaying pots values when they change
#[embassy_executor::task]
async fn task_display_pot(i2c: I2c<'static, I2C0, Async>) {
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    display.clear_buffer();
    display.flush().unwrap();

    info!("Display ready.");

    loop {
        // Wait for data
        let data = DISPLAY_CHANNEL.receive().await;

        display.clear_buffer();

        // 8-char buffer for u16
        let mut buf: String<8> = String::new();

        // Write pot0
        write!(buf, "{}", data.filtered_pot0).unwrap();
        Text::new(buf.as_str(), Point::new(0, 16), text_style)
            .draw(&mut display)
            .unwrap();
        buf.clear();
        write!(buf, "{} %", data.percentage_pot0).unwrap();
        Text::new(buf.as_str(), Point::new(32, 16), text_style)
            .draw(&mut display)
            .unwrap();
        buf.clear();
        write!(buf, "{} MIDI", data.midi_range_pot0).unwrap();
        Text::new(buf.as_str(), Point::new(64, 16), text_style)
            .draw(&mut display)
            .unwrap();

        // Write pot1
        buf.clear();
        write!(buf, "{}", data.filtered_pot1).unwrap();
        Text::new(buf.as_str(), Point::new(0, 26), text_style)
            .draw(&mut display)
            .unwrap();
        buf.clear();
        write!(buf, "{} %", data.percentage_pot1).unwrap();
        Text::new(buf.as_str(), Point::new(32, 26), text_style)
            .draw(&mut display)
            .unwrap();
        buf.clear();
        write!(buf, "{} MIDI", data.midi_range_pot1).unwrap();
        Text::new(buf.as_str(), Point::new(64, 26), text_style)
            .draw(&mut display)
            .unwrap();

        // Actually display changes
        display.flush().unwrap();
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let mut adc = Adc::new(p.ADC, Irqs, AdcConfig::default());
    let i2c = I2c::new_async(p.I2C0, p.PIN_17, p.PIN_16, Irqs, I2cConfig::default());

    // Pins declaration
    let mut _led = Output::new(p.PIN_25, Level::Low);
    let mut pot0 = Channel::new_pin(p.PIN_26, Pull::None);
    let mut pot1 = Channel::new_pin(p.PIN_27, Pull::None);

    spawner.spawn(task_display_pot(i2c).unwrap());

    // Inizialize EMA filter and other variables for ADC
    let init_pot0 = read_adc_averaged(&mut adc, &mut pot0).await;
    let init_pot1 = read_adc_averaged(&mut adc, &mut pot1).await;
    let mut ema_pot0: f32 = init_pot0 as f32;
    let mut ema_pot1: f32 = init_pot1 as f32;
    let alpha: f32 = 0.5; // 0.0 = very slow, 1.0 = no filter
    let mut last_pot0: u16 = 0;
    let mut last_pot1: u16 = 0;

    loop {
        // Async averaged adc read
        let raw_pot0 = read_adc_averaged(&mut adc, &mut pot0).await;
        let raw_pot1 = read_adc_averaged(&mut adc, &mut pot1).await;

        // Send only new values
        if last_pot0 != raw_pot0 || last_pot1 != raw_pot1 {
            last_pot0 = raw_pot0;
            last_pot1 = raw_pot1;

            // EMA filter: y[n] = α·x[n] + (1−α)·y[n−1]
            ema_pot0 = alpha * raw_pot0 as f32 + (1.0 - alpha) * ema_pot0;
            ema_pot1 = alpha * raw_pot1 as f32 + (1.0 - alpha) * ema_pot1;

            let filtered_pot0 = ema_pot0 as u16;
            let filtered_pot1 = ema_pot1 as u16;
            // TODO: hardware low pass filter. For now there is a little bit of noise and filtered value reach only 4094

            let data = PotReadings {
                filtered_pot0,
                filtered_pot1,
                percentage_pot0: filtered_to_percent(filtered_pot0),
                percentage_pot1: filtered_to_percent(filtered_pot1),
                midi_range_pot0: filtered_to_midi_range(filtered_pot0),
                midi_range_pot1: filtered_to_midi_range(filtered_pot1),
            };

            // Send latest data to channel
            if let Err(_) = DISPLAY_CHANNEL.try_send(data) {
                warn!("Channel full!");
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
