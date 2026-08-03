//! USB MIDI

use core::sync::atomic::Ordering;
use defmt::{info, warn};
use embassy_rp::{peripherals::USB, usb::Driver};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::{
    UsbDevice,
    class::midi::{Receiver, Sender},
};

use crate::{
    display::{DisplayCmd, send_display},
    state::{CHANGE_LED, PLAY_INDICATOR, PLAY_STATE},
};

/// Outgoing MIDI events, as declared in `mixxx-mapping/zapolab-mixer.midi.xml`.
#[derive(Clone, Copy)]
pub(crate) enum MidiMsg {
    ControlChange { cc: u8, value: u8 },
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
}

impl MidiMsg {
    fn to_usb_packet(self) -> [u8; 4] {
        match self {
            MidiMsg::ControlChange { cc, value } => [0x0B, 0xB0, cc, value],
            MidiMsg::NoteOn { note, velocity } => [0x09, 0x90, note, velocity],
            MidiMsg::NoteOff { note } => [0x08, 0x80, note, 0x00],
        }
    }
}

static MIDI_CHANNEL: Channel<ThreadModeRawMutex, MidiMsg, 16> = Channel::new();

/// Push a MIDI message to TX task
pub(crate) fn send_midi(msg: MidiMsg) {
    if MIDI_CHANNEL.try_send(msg).is_err() {
        warn!("MIDI channel full!");
    }
}

/// Async task running USB device
#[embassy_executor::task]
pub(crate) async fn usb_task(mut usb: UsbDevice<'static, Driver<'static, USB>>) -> ! {
    usb.run().await
}

/// Async task sending MIDI signals
#[embassy_executor::task]
pub(crate) async fn midi_tx_task(mut sender: Sender<'static, Driver<'static, USB>>) {
    loop {
        sender.wait_connection().await;
        info!("MIDI TX connected.");

        // Drop anything queued while the host was not listening.
        while MIDI_CHANNEL.try_receive().is_ok() {}

        loop {
            let msg = MIDI_CHANNEL.receive().await;
            if sender.write_packet(&msg.to_usb_packet()).await.is_err() {
                warn!("MIDI TX disconnected.");
                break;
            }
        }
    }
}

/// Applies LED feedback coming from Mixxx on the USB OUT endpoint.
#[embassy_executor::task]
pub(crate) async fn midi_rx_task(mut receiver: Receiver<'static, Driver<'static, USB>>) {
    let mut buf = [0u8; 64];
    loop {
        receiver.wait_connection().await;
        info!("MIDI RX connected.");
        loop {
            let n = match receiver.read_packet(&mut buf).await {
                Ok(n) => n,
                Err(_) => {
                    warn!("MIDI RX disconnected.");
                    break;
                }
            };

            // A transfer carries one or more 4-byte USB-MIDI event packets.
            for packet in buf[..n].chunks_exact(4) {
                let (status, note, velocity) = (packet[1], packet[2], packet[3]);

                // Velocity 0 is Note Off.
                let on = status & 0xF0 == 0x90 && velocity > 0;
                match note {
                    // play_indicator
                    0x20 | 0x21 => {
                        let deck = (note - 0x20) as usize;
                        PLAY_INDICATOR[deck].store(on, Ordering::Relaxed);
                        CHANGE_LED[deck].signal(());
                    }
                    // play
                    0x22 | 0x23 => {
                        let deck = (note - 0x22) as usize;
                        PLAY_STATE[deck].store(on, Ordering::Relaxed);
                        send_display(DisplayCmd::DrawPlayState(on, deck));
                    }
                    _ => {}
                }
            }
        }
    }
}
