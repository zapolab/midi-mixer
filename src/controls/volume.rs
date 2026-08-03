//! Potentiometers: ADC sampling and filtering

use embassy_rp::adc::{Adc, Async, Channel};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use crate::{
    display::{DisplayCmd, PotReadings, send_display},
    midi::{MidiMsg, send_midi},
};

// 0.0 = very slow, 1.0 = no filter
const ALPHA: f32 = 0.5;

pub(crate) type SharedAdc = Mutex<ThreadModeRawMutex, Adc<'static, Async>>;
static ADC: StaticCell<SharedAdc> = StaticCell::new();

/// Give the ADC
pub(crate) fn init_adc(adc: Adc<'static, Async>) -> &'static SharedAdc {
    ADC.init(Mutex::new(adc))
}

/// Read 16 values and return the average
async fn read_adc_averaged(adc: &SharedAdc, ch: &mut Channel<'_>) -> u16 {
    let mut adc = adc.lock().await;
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

/// Async task reading one potentiometer
// One instance per pot, so the pool must hold them all.
#[embassy_executor::task(pool_size = 2)]
pub(crate) async fn volume_pot_task(
    adc: &'static SharedAdc,
    mut pot: Channel<'static>,
    channel: usize,
) {
    // Inizialize EMA filter and other variables for ADC
    let mut ema: f32 = read_adc_averaged(adc, &mut pot).await as f32;
    let mut last_raw: u16 = 0;
    let mut last_midi: u8 = 0xFF;

    loop {
        // Async averaged adc read
        let raw = read_adc_averaged(adc, &mut pot).await;

        // Send only new values
        if last_raw != raw {
            last_raw = raw;

            // EMA filter: y[n] = α·x[n] + (1−α)·y[n−1]
            ema = ALPHA * raw as f32 + (1.0 - ALPHA) * ema;

            let filtered = ema as u16;
            // TODO: hardware low pass filter. For now there is a little bit of noise and filtered value reach only 4094

            let data = PotReadings {
                filtered,
                percentage: filtered_to_percent(filtered),
                midi_range: filtered_to_midi_range(filtered),
            };

            // Emit a CC only when the 7-bit value actually moves.
            if data.midi_range != last_midi {
                last_midi = data.midi_range;
                send_midi(MidiMsg::ControlChange {
                    cc: if channel == 0 { 0x00 } else { 0x01 },
                    value: data.midi_range,
                });
            }

            // Send latest data to channel
            send_display(DisplayCmd::DrawPot(data, channel));
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
