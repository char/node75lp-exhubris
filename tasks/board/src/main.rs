#![no_std]
#![no_main]

use core::{
    mem::MaybeUninit,
    ptr::{read_volatile, write_volatile},
};
use hubris_task_slots::SLOTS;
use idyll_runtime::{Leased, Meta, Read, Write};
use node75_ipc::{board::MAX_I2C_BYTES, keyboard::Node75Keyboard};
use userlib::{ReplyFaultReason, TaskId};

mod i2c;
include!(concat!(env!("OUT_DIR"), "/server.rs"));

const TICK: u32 = 1;
const PA: usize = 0x4000_10a0;
const PB: usize = 0x4000_10c0;
const RDY: u32 = 1 << 7;
const CLEAR: usize = 12;
const PULL_UP: usize = 16;
const PULL_DOWN: usize = 20;
const LED_DATA: u32 = 1 << 14;
const LED_POWER: u32 = 1 << 15;

#[unsafe(export_name = "main")]
fn main() -> ! {
    let mut server = Server {
        i2c: unsafe { i2c::I2c::new(0x44) },
        keyboard: Node75Keyboard::from(SLOTS.keyboard),
    };
    unsafe {
        let analog = 0x4000_101a as *mut u16;
        write_volatile(analog, read_volatile(analog) & !0x0008);
        update(PA, RDY, 0);
        update(PA + PULL_DOWN, RDY, 0);
        update(PA + PULL_UP, 0, RDY);
    }
    userlib::sys_set_timer(Some(userlib::sys_get_timer().now), TICK);
    let mut incoming = [MaybeUninit::uninit(); NODE_75_BOARD_BUFFER_SIZE];
    loop {
        idyll_runtime::dispatch_or_event(&mut server, TICK, &mut incoming);
    }
}

struct Server {
    i2c: i2c::I2c,
    keyboard: Node75Keyboard,
}

impl idyll_runtime::NotificationHandler for Server {
    fn handle_notification(&mut self, bits: u32) {
        if bits & TICK == 0 {
            return;
        }
        let keys = unsafe { node75_matrix::sample() };
        // USB outranks us and accepts samples without ever waiting on GPIO.
        self.keyboard
            .matrix_sample(keys, userlib::sys_get_timer().now as u32);
        let now = userlib::sys_get_timer().now;
        userlib::sys_set_timer(Some(now + 1), TICK);
    }
}

fn authorize(meta: Meta, owner: TaskId, leases: usize) -> Result<(), ReplyFaultReason> {
    if meta.sender.task_index() != owner.task_index() {
        return Err(ReplyFaultReason::AccessViolation);
    }
    if meta.lease_count != leases {
        return Err(ReplyFaultReason::BadLeases);
    }
    Ok(())
}

impl Node75Board for Server {
    fn i2c_write(
        &mut self,
        meta: Meta,
        register: u16,
        data: Leased<Read, u8>,
    ) -> Result<Result<(), BoardError>, ReplyFaultReason> {
        authorize(meta, SLOTS.touch, 1)?;
        if data.len() > MAX_I2C_BYTES {
            return Ok(Err(BoardError::TooLong));
        }
        let mut bytes = [0; MAX_I2C_BYTES];
        let bytes = &mut bytes[..data.len()];
        data.read_range(0, bytes)
            .map_err(|_| ReplyFaultReason::BadLeases)?;
        Ok(self.i2c.write(register, bytes))
    }

    fn i2c_read(
        &mut self,
        meta: Meta,
        register: u16,
        data: Leased<Write, u8>,
    ) -> Result<Result<(), BoardError>, ReplyFaultReason> {
        authorize(meta, SLOTS.touch, 1)?;
        if data.len() > MAX_I2C_BYTES {
            return Ok(Err(BoardError::TooLong));
        }
        let mut bytes = [0; MAX_I2C_BYTES];
        let bytes = &mut bytes[..data.len()];
        if let Err(error) = self.i2c.read(register, bytes) {
            return Ok(Err(error));
        }
        data.write_range(0, bytes)
            .map_err(|_| ReplyFaultReason::BadLeases)?;
        Ok(Ok(()))
    }

    fn touch_ready(&mut self, meta: Meta) -> Result<Result<bool, BoardError>, ReplyFaultReason> {
        authorize(meta, SLOTS.touch, 0)?;
        Ok(Ok(unsafe {
            read_volatile((PA + 4) as *const u32) & RDY == 0
        }))
    }

    fn set_lighting_pins(
        &mut self,
        meta: Meta,
        pins: LightingPins,
    ) -> Result<(), ReplyFaultReason> {
        authorize(meta, SLOTS.lighting, 0)?;
        let both = LED_POWER | LED_DATA;
        unsafe {
            match pins {
                // Stock enables lighting by driving the power pin low.
                LightingPins::Powered => {
                    write_volatile((PB + CLEAR) as *mut u32, both);
                    update(PB + PULL_DOWN, both, 0);
                    update(PB, 0, both);
                }
                LightingPins::Spi => {
                    let alternate = 0x4000_1018 as *mut u16;
                    write_volatile(alternate, read_volatile(alternate) | 0x100);
                }
                LightingPins::Off => {
                    write_volatile((PB + CLEAR) as *mut u32, LED_DATA);
                    for offset in [PULL_UP, PULL_DOWN, 0] {
                        update(PB + offset, LED_POWER, 0);
                    }
                }
            }
        }
        Ok(())
    }
}

unsafe fn update(address: usize, clear: u32, set: u32) {
    let register = address as *mut u32;
    unsafe { write_volatile(register, (read_volatile(register) & !clear) | set) };
}
