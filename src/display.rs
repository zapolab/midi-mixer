//! SSD1306
//! Producers never draw directly, they push a [`DisplayCmd`] through [`send_display`].

use core::{fmt::Write, sync::atomic::Ordering};
use defmt::{info, warn};
use embassy_rp::{
    i2c::{Async, I2c},
    peripherals::I2C0,
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
use heapless::String;
use ssd1306::{Ssd1306, mode::BufferedGraphicsMode, prelude::*, size::DisplaySize128x64};

use crate::state::SELECTED_DECK;

pub(crate) type Display = Ssd1306<
    I2CInterface<I2c<'static, I2C0, Async>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

/// Struct for potentiometer read values
#[derive(Clone, Copy)] // needed for not losing ownership when sending into channel
pub(crate) struct PotReadings {
    pub(crate) filtered_pot0: u16,
    pub(crate) filtered_pot1: u16,
    pub(crate) percentage_pot0: u8,
    pub(crate) percentage_pot1: u8,
    pub(crate) midi_range_pot0: u8,
    pub(crate) midi_range_pot1: u8,
}

/// Struct for potentiometer read values
pub(crate) enum DisplayCmd {
    DrawPot(PotReadings),
    DrawSelectedDeck(u8),
    DrawDirection(u8),
    DrawPlayState(bool, usize),
    DrawLoad(u8),
}

static DISPLAY_CHANNEL: Channel<ThreadModeRawMutex, DisplayCmd, 8> = Channel::new();

/// Push a draw command to the display task
pub(crate) fn send_display(cmd: DisplayCmd) {
    if DISPLAY_CHANNEL.try_send(cmd).is_err() {
        warn!("Display channel full!");
    }
}

/// Async task managing display writing requests
#[embassy_executor::task]
pub(crate) async fn display_manager_task(mut display: Display) {
    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    info!("Display ready.");

    loop {
        let cmd = DISPLAY_CHANNEL.receive().await;
        match cmd {
            DisplayCmd::DrawPot(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 0), Size::new(128, 20)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                // Write pot0
                write!(buf, "{}", data.filtered_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{}%", data.percentage_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(32, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{} MIDI", data.midi_range_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(64, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();

                // Write pot1
                buf.clear();
                write!(buf, "{}", data.filtered_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{}%", data.percentage_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(32, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{} MIDI", data.midi_range_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(64, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawSelectedDeck(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 20), Size::new(6, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 1-char buffer
                let mut buf: String<1> = String::new();

                write!(buf, "{}", data).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 20), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawDirection(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(12, 20), Size::new(24, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                if data == 1 {
                    write!(buf, "UP").unwrap();
                } else {
                    write!(buf, "DOWN").unwrap();
                }

                Text::with_baseline(buf.as_str(), Point::new(12, 20), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawPlayState(state, channel) => {
                display
                    .fill_solid(
                        &Rectangle::new(
                            if channel == 0 {
                                Point::new(0, 30)
                            } else {
                                Point::new(36, 30)
                            },
                            Size::new(30, 10),
                        ),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                write!(buf, "{}", state).unwrap();

                Text::with_baseline(
                    buf.as_str(),
                    if channel == 0 {
                        Point::new(0, 30)
                    } else {
                        Point::new(36, 30)
                    },
                    text_style,
                    Baseline::Top,
                )
                .draw(&mut display)
                .unwrap();
            }
            DisplayCmd::DrawLoad(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 40), Size::new(18, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                let deck = SELECTED_DECK.load(Ordering::Relaxed);

                write!(buf, "{} {}", data, deck).unwrap();

                Text::with_baseline(buf.as_str(), Point::new(0, 40), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
        }
        display.flush().unwrap();
    }
}
