//! Play/pause button and LED

use core::sync::atomic::Ordering;
use embassy_rp::gpio::{Input, Output};

use crate::{
    controls::shift::SHIFT_OFFSET,
    display::{DisplayCmd, send_display},
    midi::{MidiMsg, send_midi},
    state::{CHANGE_LED, PLAY_INDICATOR, SHIFT_STATE},
};

/// Async task handling play/pause button press
// One instance per deck, so the pool must hold both.
#[embassy_executor::task(pool_size = 2)]
pub(crate) async fn play_button_task(mut button: Input<'static>, channel: usize) {
    send_display(DisplayCmd::DrawPlayState(false, channel));

    loop {
        button.wait_for_low().await;
        let note = channel as u8
            + if SHIFT_STATE.load(Ordering::Relaxed) {
                SHIFT_OFFSET
            } else {
                0
            };

        send_midi(MidiMsg::NoteOn {
            note: note,
            velocity: 0x7F,
        });

        button.wait_for_high().await;
        send_midi(MidiMsg::NoteOff { note: note });
    }
}

/// Async task handling play/pause led state
// One instance per deck, so the pool must hold both.
#[embassy_executor::task(pool_size = 2)]
pub(crate) async fn play_led_task(mut led: Output<'static>, channel: usize) {
    loop {
        led.set_level(PLAY_INDICATOR[channel].load(Ordering::Relaxed).into());
        CHANGE_LED[channel].wait().await;
    }
}
