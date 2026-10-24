#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};

use node75_keyboard_core::Debouncer;
use node75_usb::{Action, Device};

mod layout;
mod usb_hw;

mod board {
    pub struct Clock;

    impl Clock {
        pub fn millis(&self) -> u32 {
            userlib::sys_get_timer().now as u32
        }
    }
}

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
    let mut last_scan = clock.millis();
    let mut keys = [0; 6];
    loop {
        let mut now = clock.millis();
        if let Some(event) = hardware.poll() {
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
                Action::Halt(halted) => hardware.set_halt(halted),
                Action::Stall => hardware.stall_control(),
            }
            unsafe { write_volatile(addr_of_mut!(USB_LEDS), device.leds()) };
            hardware.finish_event();
        }
        if now.wrapping_sub(last_scan) >= 1 {
            let raw = unsafe { node75_matrix::sample() };
            now = clock.millis();
            keys = debounce.update(raw, now);
            device.set_report(layout::report(keys));
            last_scan = now;
        }
        if let Some(report) = device.report_due(now)
            && hardware.transmit(1, &report)
        {
            device.report_queued(report);
        }
    }
}
