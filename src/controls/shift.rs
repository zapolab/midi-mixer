//! Shift button

use core::sync::atomic::Ordering;
use embassy_rp::gpio::Input;

use crate::state::SHIFT_STATE;

pub(crate) const SHIFT_OFFSET: u8 = 0x10;

#[embassy_executor::task]
pub(crate) async fn shift_button_task(mut button: Input<'static>) {
    loop {
        button.wait_for_low().await;
        SHIFT_STATE.store(true, Ordering::Relaxed);

        button.wait_for_high().await;
        SHIFT_STATE.store(false, Ordering::Relaxed);
    }
}
