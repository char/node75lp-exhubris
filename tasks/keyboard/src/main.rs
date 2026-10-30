#![no_std]
#![no_main]

use core::ptr::{addr_of_mut, read_volatile, write_volatile};

use core::mem::MaybeUninit;

use hubris_task_slots::SLOTS;
use idyll_runtime::Meta;
use node75_keyboard_core::Matrix;
use node75_keymap::Keymap;
use node75_usb::{Action, Device, Report};
use userlib::{ReplyFaultReason, TaskId};

mod usb_hw;
include!(concat!(env!("OUT_DIR"), "/server.rs"));

mod board {
    pub struct Clock;

    impl Clock {
        pub fn millis(&self) -> u32 {
            userlib::sys_get_timer().now as u32
        }
    }
}

// As task 0, the kernel reports every other task's faults on bit 0.
const FAULT: u32 = 1 << 0;
const POLL_TIMER: u32 = 1 << 1;
// A board task that stops scanning without faulting must not leave keys held.
// Board repeats the matrix at least every 100 ms.
const STALE_MS: u32 = 250;

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
    let mut keyboard = Keyboard {
        device: Device::default(),
        keymap: Keymap::new(),
        keys: [0; 6],
        sampled_at: None,
        touching: false,
        now: 0,
    };
    keyboard.device.set_telemetry(telemetry);
    let mut incoming = [MaybeUninit::uninit(); NODE_75_KEYBOARD_BUFFER_SIZE];
    loop {
        let time = userlib::sys_get_timer().now;
        let now = time as u32;
        keyboard.now = now;
        let device = &mut keyboard.device;
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
        // Every pass, not just on matrix changes: tap-hold resolves with time.
        keyboard.keymap.update(keyboard.keys, now, |resolved| {
            device.set_report(Report::new(resolved.modifiers, resolved.usages()));
        });
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
        if keyboard
            .sampled_at
            .is_some_and(|at| now.wrapping_sub(at) >= STALE_MS)
        {
            keyboard.release_inputs();
        }
        userlib::sys_set_timer(Some(time + 1), POLL_TIMER);
        idyll_runtime::dispatch_or_event(&mut keyboard, FAULT | POLL_TIMER, &mut incoming);
    }
}

struct Keyboard {
    device: Device,
    keymap: Keymap,
    keys: Matrix,
    sampled_at: Option<u32>,
    touching: bool,
    now: u32,
}

impl Keyboard {
    fn release_inputs(&mut self) {
        self.keymap = Keymap::new();
        self.keys = [0; 6];
        self.sampled_at = None;
        self.touching = false;
        self.device.set_report(Report::default());
        self.device.cancel_consumer();
    }
}

impl idyll_runtime::NotificationHandler for Keyboard {
    fn handle_notification(&mut self, bits: u32) {
        if bits & FAULT == 0 {
            return;
        }
        let mut next = 1;
        while let Some(index) = kipc::find_faulted_task(next) {
            let index = index.get();
            if index == usize::from(SLOTS.board.task_index())
                || index == usize::from(SLOTS.touch.task_index())
            {
                self.release_inputs();
            }
            kipc::reinitialize_task(index, kipc::NewState::Runnable);
            next = index + 1;
        }
    }
}

fn expect(meta: &Meta, client: TaskId) -> Result<(), ReplyFaultReason> {
    if meta.sender.task_index() == client.task_index() {
        Ok(())
    } else {
        Err(ReplyFaultReason::AccessViolation)
    }
}

impl Node75Keyboard for Keyboard {
    fn set_matrix(&mut self, meta: Meta, keys: [u16; 6]) -> Result<(), ReplyFaultReason> {
        expect(&meta, SLOTS.board)?;
        self.keys = keys;
        self.sampled_at = Some(self.now);
        Ok(())
    }

    fn media_key(&mut self, meta: Meta, usage: u16) -> Result<bool, ReplyFaultReason> {
        expect(&meta, SLOTS.touch)?;
        Ok(self.device.enqueue_consumer(usage))
    }

    fn set_contact(&mut self, meta: Meta, touching: bool) -> Result<(), ReplyFaultReason> {
        expect(&meta, SLOTS.touch)?;
        self.touching = touching;
        Ok(())
    }

    fn lighting_state(&mut self, meta: Meta) -> Result<LightingState, ReplyFaultReason> {
        expect(&meta, SLOTS.lighting)?;
        Ok(LightingState {
            keys: self.keys,
            now: self.now,
            active: self.device.active(),
            leds: self.device.leds(),
            touching: self.touching,
        })
    }
}
