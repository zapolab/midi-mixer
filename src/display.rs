//! SSD1306
//! Producers never draw directly, they push a [`DisplayCmd`] through [`send_display`].

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
    primitives::{PrimitiveStyle, Rectangle, StyledDrawable, Triangle},
    text::{Alignment, Baseline, Text},
};
use ssd1306::{Ssd1306, mode::BufferedGraphicsMode, prelude::*, size::DisplaySize128x64};

pub(crate) type Display = Ssd1306<
    I2CInterface<I2c<'static, I2C0, Async>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

/// Struct for potentiometer read values
pub(crate) enum DisplayCmd {
    DrawPot(u8, usize),
    DrawSelectedDeck(u8),
    DrawPlayState(bool, usize),
}

static DISPLAY_CHANNEL: Channel<ThreadModeRawMutex, DisplayCmd, 8> = Channel::new();

// Font styles
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

// Play/pause icon geometry
const ICON_TOP: i32 = 111;
const ICON_SIZE: Size = Size::new(12, 15);

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
            DisplayCmd::DrawPlayState(state, channel) => {
                let x = if channel == 0 { 1 } else { 51 };
                let origin = Point::new(x, ICON_TOP);
                let style = PrimitiveStyle::with_fill(BinaryColor::On);

                // Own region, cleared before every redraw
                display
                    .fill_solid(&Rectangle::new(origin, ICON_SIZE), BinaryColor::Off)
                    .unwrap();

                if state {
                    // Pause: two bars
                    Rectangle::new(origin, Size::new(5, 15))
                        .draw_styled(&style, &mut display)
                        .unwrap();
                    Rectangle::new(origin + Point::new(7, 0), Size::new(5, 15))
                        .draw_styled(&style, &mut display)
                        .unwrap();
                } else {
                    // Play: triangle
                    Triangle::new(
                        origin,
                        origin + Point::new(0, 14),
                        origin + Point::new(11, 7),
                    )
                    .draw_styled(&style, &mut display)
                    .unwrap();
                }
            }
        }
        display.flush().unwrap();
    }
}
