#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};

use node75_keyboard_core::Debouncer;
use node75_usb::{Action, Device, Report};

mod usb_hw;

mod lighting_bridge;

mod media_bridge;

mod matrix_bridge;

mod replies;

mod board {
    pub struct Clock;

    impl Clock {
        pub fn millis(&self) -> u32 {
            userlib::sys_get_timer().now as u32
        }
    }
}

// Bit 0 is reserved for supervisor fault notifications.
const POLL_TIMER: u32 = 1 << 1;

#[unsafe(no_mangle)]
static mut USB_LEDS: u8 = 0;

#[unsafe(export_name = "main")]
fn main() -> ! {
    // Clock setup belongs to the post-guard kernel handoff, not this task.
    let config = unsafe { read_volatile(0x4000_1008 as *const u16) };
    let divisor = u32::from(config & 0x1f);
    let frequency: u32 = match config & 0xc0 {
        0x00 if divisor != 0 => 32_000_000 / divisor,
        0x40 if divisor != 0 => 480_000_000 / divisor,
        0x80 => 32_000_000,
        _ => panic!("unsupported inherited clock"),
    };
    let chip_id = unsafe { read_volatile(0x4000_1041 as *const u8) };
    let mut telemetry = *b"N75D\x01\x00\x10\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    telemetry[8..12].copy_from_slice(&frequency.to_le_bytes());
    telemetry[12..16].copy_from_slice(&u32::from(chip_id).to_le_bytes());
    run(board::Clock, telemetry)
}

fn run(clock: board::Clock, telemetry: [u8; 16]) -> ! {
    let mut hardware = unsafe { usb_hw::Usb::new(&clock) };
    let mut device = Device::default();
    device.set_telemetry(telemetry);
    let mut debounce = Debouncer::default();
    let mut keymap = node75_keymap::Keymap::new();
    let mut matrix = matrix_bridge::MatrixBridge::new();
    let mut pending_matrix = None;
    let mut keys = [0; 6];
    let mut observe = lighting_bridge::observer();
    let mut touching = false;
    loop {
        let mut now = clock.millis();
        // A continuously active host must not prevent other tasks from running.
        for _ in 0..16 {
            let Some(event) = hardware.poll() else { break };
            let action = match event {
                usb_hw::Event::Reset => {
                    device.reset();
                    Action::None
                }
                usb_hw::Event::Setup(packet) => device.setup(packet),
                usb_hw::Event::Out {
                    endpoint: 0,
                    data,
                    len,
                } => device.out(&data[..len]),
                usb_hw::Event::Out { .. } => Action::None,
                usb_hw::Event::InComplete(endpoint) => device.in_complete(endpoint, now),
                usb_hw::Event::Suspend(suspended) => {
                    device.set_suspended(suspended);
                    Action::None
                }
            };
            match action {
                Action::None => {}
                Action::Send(packet) => {
                    if !hardware.transmit(0, packet.as_slice()) {
                        hardware.stall_control();
                    }
                }
                Action::Address(address) => hardware.set_address(address),
                Action::Configure(configured) => hardware.configure(configured),
                Action::Halt(endpoint, halted) => hardware.set_halt(endpoint, halted),
                Action::CancelKeyboardReport => hardware.cancel_keyboard_report(),
                Action::Stall => hardware.stall_control(),
            }
            unsafe { write_volatile(addr_of_mut!(USB_LEDS), device.leds()) };
            hardware.finish_event();
        }
        match pending_matrix.take().or_else(|| matrix.receive(now)) {
            Some(matrix_bridge::Event::Sample(sample)) => {
                keys = debounce.update(sample.keys, sample.now);
            }
            Some(matrix_bridge::Event::Lost) => {
                keys = [0; 6];
                debounce = Debouncer::default();
                keymap = node75_keymap::Keymap::new();
                device.set_report(Report::default());
            }
            None => {}
        }
        now = clock.millis();
        keymap.update(keys, now, |resolved| {
            device.set_report(Report::new(resolved.modifiers, resolved.usages()));
        });
        media_bridge::receive(&mut device, &mut touching);
        if !matrix.available() {
            device.cancel_consumer();
            touching = false;
        }
        if let Some(report) = device.report_due(now)
            && hardware.transmit(1, device.report_packet(report).as_slice())
        {
            device.report_queued(report);
        }
        if let Some(report) = device.consumer_due(now)
            && hardware.transmit(2, &report)
        {
            device.consumer_queued(u16::from_le_bytes(report));
        }
        observe(node75_lighting_api::State {
            keys,
            now,
            active: device.active(),
            leds: device.leds(),
            touching,
        });
        // Leave CPU time for optional drivers, even if they stop making syscalls.
        let time = userlib::sys_get_timer().now;
        userlib::sys_set_timer(Some(time + 1), POLL_TIMER);
        // Accept samples immediately so the board server can keep serving I2C/GPIO.
        pending_matrix = matrix.wait(POLL_TIMER);
    }
}
