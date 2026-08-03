#![no_std]
#![no_main]

mod controls;
mod display;
mod midi;
mod state;

use embassy_executor::Spawner;
use embassy_rp::{
    adc::{Adc, Channel, Config as AdcConfig, InterruptHandler as AdcInterruptHandler},
    bind_interrupts,
    gpio::{Input, Level, Output, Pull},
    i2c::{Config as I2cConfig, I2c, InterruptHandler as I2cInterruptHandler},
    peripherals::{I2C0, PIO0, USB},
    pio::{InterruptHandler as PioInterruptHandler, Pio},
    usb::{Driver, InterruptHandler as USBInterruptHandler},
};
use embassy_time::{Duration, Timer};
use embassy_usb::{Builder, Config as USBConfig, class::midi::MidiClass};
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
use ssd1306::{
    I2CDisplayInterface, Ssd1306, prelude::*, rotation::DisplayRotation, size::DisplaySize128x64,
};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use crate::{
    controls::{
        deck::deck_switch_task,
        load::{init_encoder, load_button_task, load_encoder_task},
        play::{play_button_task, play_led_task},
        volume::{init_adc, volume_pot_task},
    },
    display::display_manager_task,
    midi::{midi_rx_task, midi_tx_task, usb_task},
};

bind_interrupts!(
    struct Irqs {
        ADC_IRQ_FIFO => AdcInterruptHandler;
        I2C0_IRQ => I2cInterruptHandler<I2C0>;
        PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
        USBCTRL_IRQ => USBInterruptHandler<USB>;
    }
);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Boot sentinel
    let mut status_led = Output::new(p.PIN_25, Level::High);

    let adc = init_adc(Adc::new(p.ADC, Irqs, AdcConfig::default()));
    let i2c = I2c::new_async(p.I2C0, p.PIN_17, p.PIN_16, Irqs, I2cConfig::default());

    // Display initialization
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();
    display.clear(BinaryColor::Off).unwrap();
    display.flush().unwrap();

    // USB configuration
    let driver = Driver::new(p.USB, Irqs);
    let mut config = USBConfig::new(0x1209, 0x0001); //pid.codes test PID
    config.manufacturer = Some("Zapolab");
    config.product = Some("RP2040 Mixer");
    config.serial_number = Some("12345678");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Descriptor buffers
    static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; 256]),
        BOS_DESCRIPTOR.init([0; 256]),
        &mut [], // no msos descriptors
        CONTROL_BUF.init([0; 64]),
    );

    // Create classes on the builder: 1 embedded IN jack, 1 embedded OUT jack.
    let class = MidiClass::new(&mut builder, 1, 1, 64);
    let (midi_sender, midi_receiver) = class.split();
    let usb = builder.build();

    // Pio declaration
    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);

    // Pins declaration
    let deck_switch = Input::new(p.PIN_22, Pull::Up);
    let play_button0 = Input::new(p.PIN_14, Pull::Up);
    let play_button1 = Input::new(p.PIN_15, Pull::Up);
    let play_led0 = Output::new(p.PIN_13, Level::Low);
    let play_led1 = Output::new(p.PIN_12, Level::Low);
    let load_encoder = init_encoder(&mut common, sm0, p.PIN_2, p.PIN_3);
    let load_button = Input::new(p.PIN_21, Pull::Up);
    let volume_pot0 = Channel::new_pin(p.PIN_26, Pull::None);
    let volume_pot1 = Channel::new_pin(p.PIN_27, Pull::None);

    // Task initialization
    spawner.spawn(usb_task(usb).unwrap());
    spawner.spawn(midi_tx_task(midi_sender).unwrap());
    spawner.spawn(midi_rx_task(midi_receiver).unwrap());
    spawner.spawn(display_manager_task(display).unwrap());
    spawner.spawn(deck_switch_task(deck_switch).unwrap());
    spawner.spawn(play_button_task(play_button0, 0).unwrap());
    spawner.spawn(play_button_task(play_button1, 1).unwrap());
    spawner.spawn(play_led_task(play_led0, 0).unwrap());
    spawner.spawn(play_led_task(play_led1, 1).unwrap());
    spawner.spawn(load_encoder_task(load_encoder).unwrap());
    spawner.spawn(load_button_task(load_button).unwrap());
    spawner.spawn(volume_pot_task(adc, volume_pot0, 0).unwrap());
    spawner.spawn(volume_pot_task(adc, volume_pot1, 1).unwrap());

    // Boot done
    status_led.set_low();

    // Heartbeat
    loop {
        Timer::after(Duration::from_secs(4)).await;
        status_led.set_high();
        Timer::after(Duration::from_millis(150)).await;
        status_led.set_low();
    }
}
