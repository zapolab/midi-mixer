//! Deck-select switch

use core::sync::atomic::Ordering;
use embassy_rp::gpio::Input;

use crate::{
    display::{DisplayCmd, send_display},
    state::SELECTED_DECK,
};

/// Async task reading deck switch changes
#[embassy_executor::task]
pub(crate) async fn deck_switch_task(mut switch: Input<'static>) {
    if switch.is_high() {
        SELECTED_DECK.store(1, Ordering::Relaxed);
    }

    // Send update to channel
    let data = SELECTED_DECK.load(Ordering::Relaxed);
    send_display(DisplayCmd::DrawSelectedDeck(data));

    loop {
        switch.wait_for_any_edge().await;
        if switch.is_high() {
            SELECTED_DECK.store(1, Ordering::Relaxed);
        } else {
            SELECTED_DECK.store(0, Ordering::Relaxed);
        }

        let data = SELECTED_DECK.load(Ordering::Relaxed);
        send_display(DisplayCmd::DrawSelectedDeck(data));
    }
}
