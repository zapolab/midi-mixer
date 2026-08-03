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
    pub(crate) filtered: u16,
    pub(crate) percentage: u8,
    pub(crate) midi_range: u8,
}

/// Struct for potentiometer read values
pub(crate) enum DisplayCmd {
    DrawPot(PotReadings, usize),
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
            DisplayCmd::DrawPot(data, channel) => {
                let y = if channel == 0 { 0 } else { 10 };

                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, y), Size::new(128, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                write!(buf, "{}", data.filtered).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, y), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{}%", data.percentage).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(32, y), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{} MIDI", data.midi_range).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(64, y), text_style, Baseline::Top)
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
