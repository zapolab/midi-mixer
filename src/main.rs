#![no_std]
#![no_main]

use core::{
    fmt::Write,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::{
    Peri,
    adc::{Adc, Channel, Config as AdcConfig, InterruptHandler as AdcInterruptHandler},
    bind_interrupts,
    gpio::{self, Input, Pull},
    i2c::{Async, Config as I2cConfig, I2c, InterruptHandler as I2cInterruptHandler},
    peripherals::{I2C0, PIO0, USB},
    pio::{
        Common, Config as PioConfig, Direction as PioDirection, FifoJoin,
        InterruptHandler as PioInterruptHandler, Pio, PioPin, ShiftDirection, StateMachine,
        program,
    },
    pio_programs::clock_divider::calculate_pio_clock_divider,
    usb::{Driver, InterruptHandler as USBInterruptHandler},
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use embassy_usb::{
    Builder, Config as USBConfig, UsbDevice,
    class::midi::{MidiClass, Receiver, Sender},
};
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
use gpio::{Level, Output};
use heapless::String;
use ssd1306::{
    I2CDisplayInterface, Ssd1306, mode::BufferedGraphicsMode, prelude::*,
    rotation::DisplayRotation, size::DisplaySize128x64,
};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

/// Struct for potentiometer read values
#[derive(Clone, Copy)] // needed for not losing ownership when sending into channel
struct PotReadings {
    filtered_pot0: u16,
    filtered_pot1: u16,
    percentage_pot0: u8,
    percentage_pot1: u8,
    midi_range_pot0: u8,
    midi_range_pot1: u8,
}

/// Struct for potentiometer read values
enum DisplayCmd {
    DrawPot(PotReadings),
    DrawSelectedDeck(u8),
    DrawDirection(u8),
    DrawPlayState(bool, usize),
    DrawLoad(u8),
}

/// Outgoing MIDI events, as declared in `mixxx-mapping/zapolab-mixer.midi.xml`.
#[derive(Clone, Copy)]
enum MidiMsg {
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

// Encoder lines are sampled every ~5 µs
const ENCODER_PIO_CLOCK_HZ: u32 = 1_000_000;
const QUARTER_STEPS_PER_DETENT: i8 = 2;
// Used for ignore bounces on direction jumps
const QUADRATURE_TABLE: [i8; 16] = [
    0, 1, -1, 0, //
    -1, 0, 0, 1, //
    1, 0, 0, -1, //
    0, -1, 1, 0, //
];

static MIDI_CHANNEL: embassy_sync::channel::Channel<ThreadModeRawMutex, MidiMsg, 16> =
    embassy_sync::channel::Channel::new();
static DISPLAY_CHANNEL: embassy_sync::channel::Channel<ThreadModeRawMutex, DisplayCmd, 8> =
    embassy_sync::channel::Channel::new();
static SELECTED_DECK: AtomicU8 = AtomicU8::new(0);
static PLAY_STATE: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
// Mixxx's play_indicator LED
static PLAY_INDICATOR: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
static CHANGE_LED: [Signal<ThreadModeRawMutex, ()>; 2] = [Signal::new(), Signal::new()];

bind_interrupts!(
    struct Irqs {
        ADC_IRQ_FIFO => AdcInterruptHandler;
        I2C0_IRQ => I2cInterruptHandler<I2C0>;
        PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
        USBCTRL_IRQ => USBInterruptHandler<USB>;
    }
);

/// Push a MIDI message to TX task
fn send_midi(msg: MidiMsg) {
    if MIDI_CHANNEL.try_send(msg).is_err() {
        warn!("MIDI channel full!");
    }
}

/// Starts the state machine for the encoder
// Logic by AI
fn init_encoder(
    common: &mut Common<'static, PIO0>,
    mut sm: StateMachine<'static, PIO0, 0>,
    pin_clk: Peri<'static, impl PioPin>,
    pin_dt: Peri<'static, impl PioPin>,
) -> StateMachine<'static, PIO0, 0> {
    let prg = program::pio_asm!(
        "start:",
        "    mov isr, null",
        "    in pins, 2",
        "    mov x, isr",
        "    jmp x!=y, changed",
        "    jmp start",
        "changed:",
        "    mov y, x",
        "    push",
    );
    let prg = common.load_program(&prg.program);

    let mut pin_clk = common.make_pio_pin(pin_clk);
    let mut pin_dt = common.make_pio_pin(pin_dt);
    pin_clk.set_pull(Pull::Up);
    pin_dt.set_pull(Pull::Up);
    sm.set_pin_dirs(PioDirection::In, &[&pin_clk, &pin_dt]);

    let mut cfg = PioConfig::default();
    cfg.set_in_pins(&[&pin_clk, &pin_dt]);
    cfg.fifo_join = FifoJoin::RxOnly;
    cfg.shift_in.direction = ShiftDirection::Left;
    cfg.clock_divider = calculate_pio_clock_divider(ENCODER_PIO_CLOCK_HZ);
    cfg.use_program(&prg, &[]);

    sm.set_config(&cfg);
    sm.set_enable(true);
    sm
}

/// Read 16 values and return the average
async fn read_adc_averaged(adc: &mut Adc<'_, embassy_rp::adc::Async>, ch: &mut Channel<'_>) -> u16 {
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

/// Async task managing display writing requests
#[embassy_executor::task]
async fn display_manager_task(
    mut display: Ssd1306<
        I2CInterface<I2c<'static, I2C0, Async>>,
        DisplaySize128x64,
        BufferedGraphicsMode<DisplaySize128x64>,
    >,
) {
    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    info!("Display ready.");

    loop {
        let cmd = DISPLAY_CHANNEL.receive().await;
        match cmd {
            DisplayCmd::DrawPot(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 0), Size::new(128, 20)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                // Write pot0
                write!(buf, "{}", data.filtered_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{}%", data.percentage_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(32, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{} MIDI", data.midi_range_pot0).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(64, 0), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();

                // Write pot1
                buf.clear();
                write!(buf, "{}", data.filtered_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{}%", data.percentage_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(32, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
                buf.clear();
                write!(buf, "{} MIDI", data.midi_range_pot1).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(64, 10), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawSelectedDeck(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 20), Size::new(6, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 1-char buffer
                let mut buf: String<1> = String::new();

                write!(buf, "{}", data).unwrap();
                Text::with_baseline(buf.as_str(), Point::new(0, 20), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawDirection(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(12, 20), Size::new(24, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                if data == 1 {
                    write!(buf, "UP").unwrap();
                } else {
                    write!(buf, "DOWN").unwrap();
                }

                Text::with_baseline(buf.as_str(), Point::new(12, 20), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
            DisplayCmd::DrawPlayState(state, channel) => {
                display
                    .fill_solid(
                        &Rectangle::new(
                            if channel == 0 {
                                Point::new(0, 30)
                            } else {
                                Point::new(36, 30)
                            },
                            Size::new(30, 10),
                        ),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                write!(buf, "{}", state).unwrap();

                Text::with_baseline(
                    buf.as_str(),
                    if channel == 0 {
                        Point::new(0, 30)
                    } else {
                        Point::new(36, 30)
                    },
                    text_style,
                    Baseline::Top,
                )
                .draw(&mut display)
                .unwrap();
            }
            DisplayCmd::DrawLoad(data) => {
                display
                    .fill_solid(
                        &Rectangle::new(Point::new(0, 40), Size::new(18, 10)),
                        BinaryColor::Off,
                    )
                    .unwrap();

                // 8-char buffer
                let mut buf: String<8> = String::new();

                let deck = SELECTED_DECK.load(Ordering::Relaxed);

                write!(buf, "{} {}", data, deck).unwrap();

                Text::with_baseline(buf.as_str(), Point::new(0, 40), text_style, Baseline::Top)
                    .draw(&mut display)
                    .unwrap();
            }
        }
        display.flush().unwrap();
    }
}

/// Async task reading deck switch changes
#[embassy_executor::task]
async fn deck_switch_task(mut switch: Input<'static>) {
    if switch.is_high() {
        SELECTED_DECK.store(1, Ordering::Relaxed);
    }

    // Send update to channel
    let data = SELECTED_DECK.load(Ordering::Relaxed);
    if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawSelectedDeck(data)) {
        warn!("Channel full!");
    }

    loop {
        switch.wait_for_any_edge().await;
        if switch.is_high() {
            SELECTED_DECK.store(1, Ordering::Relaxed);
        } else {
            SELECTED_DECK.store(0, Ordering::Relaxed);
        }

        let data = SELECTED_DECK.load(Ordering::Relaxed);
        if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawSelectedDeck(data)) {
            warn!("Channel full!");
        }
    }
}

/// Async task reading the rotary encoder
// Function generated by AI
#[embassy_executor::task]
async fn load_encoder_task(mut sm: StateMachine<'static, PIO0, 0>) {
    let mut state = (sm.rx().wait_pull().await & 0b11) as u8;
    let mut quarter: i8 = 0;

    info!("Encoder ready.");

    loop {
        let new_state = (sm.rx().wait_pull().await & 0b11) as u8;
        let delta = QUADRATURE_TABLE[((state << 2) | new_state) as usize];
        state = new_state;

        if delta == 0 {
            continue;
        }
        quarter += delta;

        if quarter.abs() < QUARTER_STEPS_PER_DETENT {
            continue;
        }
        let data = if quarter > 0 { 1 } else { 0 };
        quarter = 0;

        // CC 0x10, 0x41 up, 0x3F down.
        send_midi(MidiMsg::ControlChange {
            cc: 0x10,
            value: if data == 1 { 0x41 } else { 0x3F },
        });

        if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawDirection(data)) {
            warn!("Channel full!");
        }
    }
}

/// Async task reading load button press
#[embassy_executor::task]
async fn load_button_task(mut button: Input<'static>) {
    // Actual logic will send MIDI Note On/Off signal

    if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawLoad(0)) {
        warn!("Channel full!");
    }

    loop {
        button.wait_for_low().await;
        // Note 0x02 or 0x03
        let note = 0x02 + SELECTED_DECK.load(Ordering::Relaxed);
        send_midi(MidiMsg::NoteOn {
            note,
            velocity: 0x7F,
        });

        if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawLoad(1)) {
            warn!("Channel full!");
        }

        button.wait_for_high().await;
        send_midi(MidiMsg::NoteOff { note });

        if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawLoad(0)) {
            warn!("Channel full!");
        }
    }
}

/// Async task handling play/pause button press
// One instance per deck, so the pool must hold both.
#[embassy_executor::task(pool_size = 2)]
async fn play_button_task(mut button: Input<'static>, channel: usize) {
    if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawPlayState(false, channel)) {
        warn!("Channel full!");
    }

    loop {
        button.wait_for_low().await;
        send_midi(MidiMsg::NoteOn {
            note: channel as u8,
            velocity: 0x7F,
        });

        button.wait_for_high().await;
        send_midi(MidiMsg::NoteOff {
            note: channel as u8,
        });
    }
}

/// Async task handling play/pause led state
// One instance per deck, so the pool must hold both.
#[embassy_executor::task(pool_size = 2)]
async fn play_led_task(mut led: Output<'static>, channel: usize) {
    loop {
        led.set_level(PLAY_INDICATOR[channel].load(Ordering::Relaxed).into());
        CHANGE_LED[channel].wait().await;
    }
}

/// Async task running USB device
#[embassy_executor::task]
async fn usb_task(mut usb: UsbDevice<'static, Driver<'static, USB>>) -> ! {
    usb.run().await
}

/// Async task sending MIDI signals
#[embassy_executor::task]
async fn midi_tx_task(mut sender: Sender<'static, Driver<'static, USB>>) {
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
async fn midi_rx_task(mut receiver: Receiver<'static, Driver<'static, USB>>) {
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
                        if DISPLAY_CHANNEL
                            .try_send(DisplayCmd::DrawPlayState(on, deck))
                            .is_err()
                        {
                            warn!("Channel full!");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Boot sentinel
    let mut status_led = Output::new(p.PIN_25, Level::High);

    let mut adc = Adc::new(p.ADC, Irqs, AdcConfig::default());
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
    let mut volume_pot0 = Channel::new_pin(p.PIN_26, Pull::None);
    let mut volume_pot1 = Channel::new_pin(p.PIN_27, Pull::None);

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

    // Inizialize EMA filter and other variables for ADC
    let init_pot0 = read_adc_averaged(&mut adc, &mut volume_pot0).await;
    let init_pot1 = read_adc_averaged(&mut adc, &mut volume_pot1).await;
    let mut ema_pot0: f32 = init_pot0 as f32;
    let mut ema_pot1: f32 = init_pot1 as f32;
    let alpha: f32 = 0.5; // 0.0 = very slow, 1.0 = no filter
    let mut last_pot0: u16 = 0;
    let mut last_pot1: u16 = 0;

    // Boot done
    status_led.set_low();

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

            // Send latest data to channel
            if let Err(_) = DISPLAY_CHANNEL.try_send(DisplayCmd::DrawPot(data)) {
                warn!("Channel full!");
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
