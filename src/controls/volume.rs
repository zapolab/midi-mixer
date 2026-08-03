//! Volume potentiometers: ADC sampling and filtering
//!
//! Unlike the other controls this is not spawned as a task — `main` owns the
//! ADC and awaits [`run`] as its final act.

use embassy_rp::adc::{Adc, Async, Channel};
use embassy_time::{Duration, Timer};

use crate::{
    display::{DisplayCmd, PotReadings, send_display},
    midi::{MidiMsg, send_midi},
};

/// Read 16 values and return the average
async fn read_adc_averaged(adc: &mut Adc<'_, Async>, ch: &mut Channel<'_>) -> u16 {
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

/// Drives the potentiometer sampling loop (50 ms period).
pub(crate) async fn run(
    mut adc: Adc<'static, Async>,
    mut volume_pot0: Channel<'static>,
    mut volume_pot1: Channel<'static>,
) -> ! {
    // Inizialize EMA filter and other variables for ADC
    let init_pot0 = read_adc_averaged(&mut adc, &mut volume_pot0).await;
    let init_pot1 = read_adc_averaged(&mut adc, &mut volume_pot1).await;
    let mut ema_pot0: f32 = init_pot0 as f32;
    let mut ema_pot1: f32 = init_pot1 as f32;
    let alpha: f32 = 0.5; // 0.0 = very slow, 1.0 = no filter
    let mut last_pot0: u16 = 0;
    let mut last_pot1: u16 = 0;
    let mut last_midi_pot0: u8 = 0xFF;
    let mut last_midi_pot1: u8 = 0xFF;

    loop {
        // Async averaged adc read
        let raw_pot0 = read_adc_averaged(&mut adc, &mut volume_pot0).await;
        let raw_pot1 = read_adc_averaged(&mut adc, &mut volume_pot1).await;

        // Send only new values
        if last_pot0 != raw_pot0 || last_pot1 != raw_pot1 {
            last_pot0 = raw_pot0;
            last_pot1 = raw_pot1;

            // EMA filter: y[n] = α·x[n] + (1−α)·y[n−1]
            ema_pot0 = alpha * raw_pot0 as f32 + (1.0 - alpha) * ema_pot0;
            ema_pot1 = alpha * raw_pot1 as f32 + (1.0 - alpha) * ema_pot1;

            let filtered_pot0 = ema_pot0 as u16;
            let filtered_pot1 = ema_pot1 as u16;
            // TODO: hardware low pass filter. For now there is a little bit of noise and filtered value reach only 4094

            let data = PotReadings {
                filtered_pot0,
                filtered_pot1,
                percentage_pot0: filtered_to_percent(filtered_pot0),
                percentage_pot1: filtered_to_percent(filtered_pot1),
                midi_range_pot0: filtered_to_midi_range(filtered_pot0),
                midi_range_pot1: filtered_to_midi_range(filtered_pot1),
            };

            // Emit a CC only when the 7-bit value actually moves.
            if data.midi_range_pot0 != last_midi_pot0 {
                last_midi_pot0 = data.midi_range_pot0;
                send_midi(MidiMsg::ControlChange {
                    cc: 0x00,
                    value: data.midi_range_pot0,
                });
            }
            if data.midi_range_pot1 != last_midi_pot1 {
                last_midi_pot1 = data.midi_range_pot1;
                send_midi(MidiMsg::ControlChange {
                    cc: 0x01,
                    value: data.midi_range_pot1,
                });
            }

            // Send latest data to channel
            send_display(DisplayCmd::DrawPot(data));
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
