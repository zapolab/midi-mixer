//! Potentiometers: ADC sampling and filtering

use embassy_rp::adc::{Adc, Async, Channel};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use crate::{
    display::{DisplayCmd, send_display},
    midi::{MidiMsg, send_midi},
};

// 0.0 = very slow, 1.0 = no filter
const ALPHA: f32 = 0.5;

// Samples averaged per reading
const SAMPLES: u32 = 16;

// Mapping constants
const RAW_MIN: u16 = 40;
const RAW_MAX: u16 = 4080;
const _: () = assert!(RAW_MIN < RAW_MAX);

pub(crate) type SharedAdc = Mutex<ThreadModeRawMutex, Adc<'static, Async>>;
static ADC: StaticCell<SharedAdc> = StaticCell::new();

/// Give the ADC
pub(crate) fn init_adc(adc: Adc<'static, Async>) -> &'static SharedAdc {
    ADC.init(Mutex::new(adc))
}

/// Read `SAMPLES` values and return the rounded average
async fn read_adc_averaged(adc: &SharedAdc, ch: &mut Channel<'_>) -> u16 {
    let mut adc = adc.lock().await;
    let mut sum: u32 = 0;
    for _ in 0..SAMPLES {
        let val = adc.read(ch).await.unwrap();
        sum += val as u32;
        Timer::after_micros(100).await;
    }
    ((sum + SAMPLES / 2) / SAMPLES) as u16
}

/// Converts ADC filtered to 0–127 MIDI range
fn filtered_to_midi_range(raw: u16) -> u8 {
    ((raw as u32 * 127) / 4095).min(127) as u8
}

/// Map raw value in 0-4095 range
fn map_raw(raw: u16) -> u16 {
    let r = raw.clamp(RAW_MIN, RAW_MAX);
    (((r - RAW_MIN) as u32 * 4095) / (RAW_MAX - RAW_MIN) as u32) as u16
}

/// Async task reading one potentiometer
// One instance per pot, so the pool must hold them all.
#[embassy_executor::task(pool_size = 2)]
pub(crate) async fn volume_pot_task(
    adc: &'static SharedAdc,
    mut pot: Channel<'static>,
    channel: usize,
) {
    // Inizialize EMA filter and other variables for ADC
    let mut ema: f32 = map_raw(read_adc_averaged(adc, &mut pot).await) as f32;
    let mut last_filtered: u16 = u16::MAX;
    let mut last_midi: u8 = 0xFF;

    loop {
        // Async averaged adc read
        let raw = read_adc_averaged(adc, &mut pot).await;
        let mapped = map_raw(raw);

        // EMA filter: y[n] = α·x[n] + (1−α)·y[n−1].
        ema = ALPHA * mapped as f32 + (1.0 - ALPHA) * ema;

        let filtered = (ema + 0.5) as u16;
        // TODO: hardware low pass filter, for now there is a little bit of noise

        // Ignore unchanged values
        if filtered != last_filtered {
            last_filtered = filtered;

            let data = filtered_to_midi_range(filtered);

            // Emit a CC only when the 7-bit value moves
            if data != last_midi {
                last_midi = data;
                send_midi(MidiMsg::ControlChange {
                    cc: if channel == 0 { 0x00 } else { 0x01 },
                    value: data,
                });
                send_display(DisplayCmd::DrawPot(data, channel));
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
