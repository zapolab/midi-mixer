//! Cross-task shared states

use core::sync::atomic::{AtomicBool, AtomicU8};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};

/// Deck in which load the track
pub(crate) static SELECTED_DECK: AtomicU8 = AtomicU8::new(0);
/// Play/pause state per deck from Mixxx
pub(crate) static PLAY_STATE: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
/// play_indicator LED from Mixxx
pub(crate) static PLAY_INDICATOR: [AtomicBool; 2] =
    [AtomicBool::new(false), AtomicBool::new(false)];
/// Wakes the matching `play_led_task` after `PLAY_INDICATOR` changes
pub(crate) static CHANGE_LED: [Signal<ThreadModeRawMutex, ()>; 2] = [Signal::new(), Signal::new()];
