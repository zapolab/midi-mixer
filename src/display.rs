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
    mono_font::{
        MonoTextStyle, MonoTextStyleBuilder,
        ascii::{FONT_6X10, FONT_9X15_BOLD, FONT_10X20},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, StyledDrawable},
    text::{Alignment, Baseline, Text},
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
pub(crate) enum DisplayCmd {
    DrawPot(u8, usize),
    DrawSelectedDeck(u8),
    DrawDirection(u8),
    DrawPlayState(bool, usize),
    DrawLoad(u8),
}

static DISPLAY_CHANNEL: Channel<ThreadModeRawMutex, DisplayCmd, 8> = Channel::new();

const TITLE_STYLE: MonoTextStyle<'_, BinaryColor> = MonoTextStyleBuilder::new()
    .font(&FONT_10X20)
    .text_color(BinaryColor::On)
    .build();
const MEDIUM_STYLE: MonoTextStyle<'_, BinaryColor> = MonoTextStyleBuilder::new()
    .font(&FONT_9X15_BOLD)
    .text_color(BinaryColor::On)
    .build();
const SMALL_STYLE: MonoTextStyle<'_, BinaryColor> = MonoTextStyleBuilder::new()
    .font(&FONT_6X10)
    .text_color(BinaryColor::On)
    .build();

// Volume bar geometry
const VOLUME_BAR_TOP: i32 = 26;
const VOLUME_BAR_WIDTH: u32 = 10;
const VOLUME_BAR_HEIGHT: u32 = 76;

/// Push a draw command to the display task
pub(crate) fn send_display(cmd: DisplayCmd) {
    if DISPLAY_CHANNEL.try_send(cmd).is_err() {
        warn!("Display channel full!");
    }
}

/// Draw boot graphics onto the display
pub(crate) fn draw_splash(target: &mut Display) {
    target.clear(BinaryColor::Off).unwrap();
    target.flush().unwrap();

    Text::with_baseline("zapolab", Point::new(0, 43), MEDIUM_STYLE, Baseline::Top)
        .draw(target)
        .unwrap();
    Text::with_alignment(
        "RP2040\nMIDI mixer",
        Point::new(32, 76),
        SMALL_STYLE,
        Alignment::Center,
    )
    .draw(target)
    .unwrap();

    target.flush().unwrap();
}

/// Draw mixer layout onto the display
fn draw_mixer(target: &mut Display) {
    target.clear(BinaryColor::Off).unwrap();

    Text::with_baseline("1", Point::new(2, 0), TITLE_STYLE, Baseline::Top)
        .draw(target)
        .unwrap();
    Text::with_baseline("M", Point::new(27, 0), TITLE_STYLE, Baseline::Top)
        .draw(target)
        .unwrap();
    Text::with_baseline("2", Point::new(52, 0), TITLE_STYLE, Baseline::Top)
        .draw(target)
        .unwrap();

    Rectangle::new(Point::new(0, 24), Size::new(14, 80))
        .draw_styled(&PrimitiveStyle::with_stroke(BinaryColor::On, 1), target)
        .unwrap();
    Rectangle::new(Point::new(25, 24), Size::new(14, 50))
        .draw_styled(&PrimitiveStyle::with_stroke(BinaryColor::On, 1), target)
        .unwrap();
    Rectangle::new(Point::new(50, 24), Size::new(14, 80))
        .draw_styled(&PrimitiveStyle::with_stroke(BinaryColor::On, 1), target)
        .unwrap();

    target.flush().unwrap();
}

/// Async task managing display writing requests
#[embassy_executor::task]
pub(crate) async fn display_manager_task(mut display: Display) {
    draw_mixer(&mut display);

    info!("Display ready.");

    loop {
        let cmd = DISPLAY_CHANNEL.receive().await;
        match cmd {
            DisplayCmd::DrawPot(data, channel) => {
                let x = if channel == 0 { 2 } else { 52 };

                // Map MIDI value to VOLUME_BAR_HEIGHT pixels
                let filled = (data as u32 * VOLUME_BAR_HEIGHT / 127).min(VOLUME_BAR_HEIGHT);
                let empty = VOLUME_BAR_HEIGHT - filled;

                display
                    .fill_solid(
                        &Rectangle::new(
                            Point::new(x, VOLUME_BAR_TOP),
                            Size::new(VOLUME_BAR_WIDTH, VOLUME_BAR_HEIGHT),
                        ),
                        BinaryColor::On,
                    )
                    .unwrap();

                display
                    .fill_solid(
                        &Rectangle::new(
                            Point::new(x, VOLUME_BAR_TOP),
                            Size::new(VOLUME_BAR_WIDTH, empty),
                        ),
                        BinaryColor::Off,
                    )
                    .unwrap();
            }
            DisplayCmd::DrawSelectedDeck(data) => {
                let s = if data == 0 { "<<<" } else { ">>>" };

                display
                    .fill_solid(
                        &Rectangle::new(Point::new(19, 85), Size::new(27, 15)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                Text::with_baseline(s, Point::new(19, 85), MEDIUM_STYLE, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawDirection(data) => {
                // display
                //     .fill_solid(
                //         &Rectangle::new(Point::new(12, 20), Size::new(24, 10)),
                //         BinaryColor::Off,
                //     )
                //     .unwrap();

                // // 8-char buffer
                // let mut buf: String<8> = String::new();

                // if data == 1 {
                //     write!(buf, "UP").unwrap();
                // } else {
                //     write!(buf, "DOWN").unwrap();
                // }

                // Text::with_baseline(buf.as_str(), Point::new(12, 20), text_style, Baseline::Top)
                //     .draw(&mut display)
                //     .unwrap();
            }
            DisplayCmd::DrawPlayState(state, channel) => {
                // display
                //     .fill_solid(
                //         &Rectangle::new(
                //             if channel == 0 {
                //                 Point::new(0, 30)
                //             } else {
                //                 Point::new(36, 30)
                //             },
                //             Size::new(30, 10),
                //         ),
                //         BinaryColor::Off,
                //     )
                //     .unwrap();

                // // 8-char buffer
                // let mut buf: String<8> = String::new();

                // write!(buf, "{}", state).unwrap();

                // Text::with_baseline(
                //     buf.as_str(),
                //     if channel == 0 {
                //         Point::new(0, 30)
                //     } else {
                //         Point::new(36, 30)
                //     },
                //     text_style,
                //     Baseline::Top,
                // )
                // .draw(&mut display)
                // .unwrap();
            }
            DisplayCmd::DrawLoad(data) => {
                // display
                //     .fill_solid(
                //         &Rectangle::new(Point::new(0, 40), Size::new(18, 10)),
                //         BinaryColor::Off,
                //     )
                //     .unwrap();

                // // 8-char buffer
                // let mut buf: String<8> = String::new();

                // let deck = SELECTED_DECK.load(Ordering::Relaxed);

                // write!(buf, "{} {}", data, deck).unwrap();

                // Text::with_baseline(buf.as_str(), Point::new(0, 40), text_style, Baseline::Top)
                //     .draw(&mut display)
                //     .unwrap();
            }
        }
        display.flush().unwrap();
    }
}
