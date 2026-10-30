#![no_std]
#![no_main]

use core::{
    mem::MaybeUninit,
    ptr::{read_volatile, write_volatile},
};
use hubris_task_slots::SLOTS;
use idyll_runtime::{Leased, Meta, Read, Write};
use node75_board_api::{GpioWrite, MAX_I2C_BYTES, MAX_WRITES, Register};
use node75_keyboard_core::{MATRIX_SAMPLE, MatrixSample};
use userlib::{ReplyFaultReason, TaskId};

mod i2c;
include!(concat!(env!("OUT_DIR"), "/server.rs"));

const TICK: u32 = 1;
const PA: usize = 0x4000_10a0;
const PB: usize = 0x4000_10c0;
const RDY: u32 = 1 << 7;

#[unsafe(export_name = "main")]
fn main() -> ! {
    let mut server = Server {
        i2c: unsafe { i2c::I2c::new(0x44) },
        keyboard: SLOTS.keyboard,
    };
    unsafe {
        let analog = 0x4000_101a as *mut u16;
        write_volatile(analog, read_volatile(analog) & !0x0008);
        for (offset, clear, set) in [(0, RDY, 0), (20, RDY, 0), (16, 0, RDY)] {
            let register = (PA + offset) as *mut u32;
            write_volatile(register, (read_volatile(register) & !clear) | set);
        }
    }
    userlib::sys_set_timer(Some(userlib::sys_get_timer().now), TICK);
    let mut incoming = [MaybeUninit::uninit(); NODE_75_BOARD_BUFFER_SIZE];
    loop {
        idyll_runtime::dispatch_or_event(&mut server, TICK, &mut incoming);
    }
}

struct Server {
    i2c: i2c::I2c,
    keyboard: TaskId,
}

impl idyll_runtime::NotificationHandler for Server {
    fn handle_notification(&mut self, bits: u32) {
        if bits & TICK == 0 {
            return;
        }
        let keys = unsafe { node75_matrix::sample() };
        let sample = MatrixSample {
            keys,
            now: userlib::sys_get_timer().now as u32,
        };
        // USB outranks us. It accepts pending samples without ever waiting on GPIO.
        if let Err(dead) = userlib::sys_send(
            self.keyboard,
            MATRIX_SAMPLE,
            &sample.encode(),
            &mut [],
            &mut [],
        ) {
            self.keyboard = self.keyboard.with_generation(dead.new_generation());
        }
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

    fn apply_writes(
        &mut self,
        meta: Meta,
        writes: Leased<Read, GpioWrite>,
    ) -> Result<Result<(), BoardError>, ReplyFaultReason> {
        authorize(meta, SLOTS.lighting, 1)?;
        if writes.len() > MAX_WRITES {
            return Ok(Err(BoardError::TooLong));
        }
        let mut batch = [GpioWrite::new(Register::DirectionB, 0, 0); MAX_WRITES];
        let batch = &mut batch[..writes.len()];
        writes
            .read_range(0, batch)
            .map_err(|_| ReplyFaultReason::BadLeases)?;
        // Reject the entire batch before touching hardware, using our own stable copy.
        if batch.iter().any(|write| write.register().is_none()) {
            return Err(ReplyFaultReason::BadMessageContents);
        }
        for write in batch {
            unsafe {
                match write.register().expect("validated batch") {
                    Register::PinAlternate => {
                        let register = 0x4000_1018 as *mut u16;
                        write_volatile(
                            register,
                            (read_volatile(register) & !(write.clear as u16)) | write.set as u16,
                        );
                    }
                    Register::ClearB => write_volatile((PB + 12) as *mut u32, write.set),
                    register => {
                        let offset = match register {
                            Register::DirectionB => 0,
                            Register::PullUpB => 16,
                            Register::PullDownB => 20,
                            _ => unreachable!(),
                        };
                        let register = (PB + offset) as *mut u32;
                        write_volatile(
                            register,
                            (read_volatile(register) & !write.clear) | write.set,
                        );
                    }
                }
            }
        }
        Ok(Ok(()))
    }
}
