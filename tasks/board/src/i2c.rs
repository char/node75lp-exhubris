//! Open-drain I2C on PB20/PB21. The board task owns every GPIO writer;
//! USB can preempt transactions without touching these registers.

use core::ptr::{read_volatile, write_volatile};

const PB: usize = 0x4000_10c0;
const INPUT: usize = 4;
const CLEAR: usize = 12;
const PULL_UP: usize = 16;
const PULL_DOWN: usize = 20;
const SDA: u32 = 1 << 20;
const SCL: u32 = 1 << 21;
const TMR0: usize = 0x4000_2000;
const TIMER_WRAP: u32 = 1 << 25;
// Fast mode's 1.3 µs minimum SCL low time, in nanoseconds; code between edges
// counts toward it.
const HALF_BIT_NS: u32 = 1_300;
// Shared across every clock edge, including STOP; not a per-bit timeout.
const STRETCH_US: u32 = 20_000;

use crate::BoardError as Error;

pub struct I2c {
    address: u8,
    half_bit: u32,
    stretch_limit: u32,
    edge: u32,
    transaction_start: u32,
}

impl I2c {
    /// Requires exclusive access to PB20/PB21/TMR0.
    pub unsafe fn new(seven_bit_address: u8, cycles_per_us: u32) -> Self {
        unsafe {
            write_volatile(TMR0 as *mut u8, 0x02);
            write_volatile((TMR0 + 0x0c) as *mut u32, TIMER_WRAP);
            write_volatile(TMR0 as *mut u8, 0x04);
            let dir = PB as *mut u32;
            let pull_up = (PB + PULL_UP) as *mut u32;
            let pull_down = (PB + PULL_DOWN) as *mut u32;
            write_volatile(dir, read_volatile(dir) & !(SDA | SCL));
            write_volatile((PB + CLEAR) as *mut u32, SDA | SCL);
            write_volatile(pull_down, read_volatile(pull_down) & !(SDA | SCL));
            write_volatile(pull_up, read_volatile(pull_up) | SDA | SCL);
        }
        Self {
            address: seven_bit_address << 1,
            half_bit: (HALF_BIT_NS * cycles_per_us).div_ceil(1_000),
            stretch_limit: STRETCH_US * cycles_per_us,
            edge: 0,
            transaction_start: 0,
        }
    }

    pub fn write(&mut self, register: u16, data: &[u8]) -> Result<(), Error> {
        self.transaction(|bus| {
            bus.address_phase(register)?;
            for &byte in data {
                bus.write_byte(byte)?;
            }
            Ok(())
        })
    }

    pub fn read(&mut self, register: u16, data: &mut [u8]) -> Result<(), Error> {
        self.transaction(|bus| {
            bus.address_phase(register)?;
            bus.start()?;
            bus.write_byte(bus.address | 1)?;
            let last = data.len().saturating_sub(1);
            for (i, byte) in data.iter_mut().enumerate() {
                *byte = bus.read_byte(i != last)?;
            }
            Ok(())
        })
    }

    fn transaction(
        &mut self,
        transfer: impl FnOnce(&mut Self) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.transaction_start = cycles();
        self.edge = self.transaction_start;
        let result = self.start().and_then(|()| transfer(self));
        let stop = self.stop();
        result.and(stop)
    }

    fn address_phase(&mut self, register: u16) -> Result<(), Error> {
        self.write_byte(self.address)?;
        if register < 0x100 {
            self.write_byte(register as u8)?;
        } else {
            self.write_byte((register >> 8) as u8)?;
            self.write_byte(register as u8)?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), Error> {
        self.release(SDA);
        self.wait();
        self.release_clock()?;
        self.wait();
        if !self.read_pin(SDA) {
            return Err(Error::BusBusy);
        }
        self.drive_low(SDA);
        self.wait();
        self.drive_low(SCL);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.drive_low(SCL);
        self.drive_low(SDA);
        self.wait();
        let result = self.release_clock();
        self.wait();
        self.release(SDA);
        self.wait();
        result
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), Error> {
        for bit in (0..8).rev() {
            if byte & (1 << bit) != 0 {
                self.release(SDA);
            } else {
                self.drive_low(SDA);
            }
            self.wait();
            self.release_clock()?;
            self.wait();
            self.drive_low(SCL);
        }
        self.release(SDA);
        self.wait();
        self.release_clock()?;
        self.wait();
        let ack = !self.read_pin(SDA);
        self.drive_low(SCL);
        if ack { Ok(()) } else { Err(Error::Nack) }
    }

    fn read_byte(&mut self, ack: bool) -> Result<u8, Error> {
        self.release(SDA);
        let mut byte = 0u8;
        for _ in 0..8 {
            self.wait();
            self.release_clock()?;
            self.wait();
            byte = (byte << 1) | u8::from(self.read_pin(SDA));
            self.drive_low(SCL);
        }
        if ack {
            self.drive_low(SDA);
        }
        self.wait();
        self.release_clock()?;
        self.wait();
        self.drive_low(SCL);
        self.release(SDA);
        Ok(byte)
    }

    fn release_clock(&mut self) -> Result<(), Error> {
        self.release(SCL);
        while !self.read_pin(SCL) {
            if since(self.transaction_start) >= self.stretch_limit {
                return Err(Error::ClockTimeout);
            }
        }
        // A stretched clock's high time starts when the target releases it.
        self.edge = cycles();
        Ok(())
    }

    fn drive_low(&mut self, pin: u32) {
        unsafe {
            let dir = PB as *mut u32;
            write_volatile(dir, read_volatile(dir) | pin);
        }
    }

    fn release(&mut self, pin: u32) {
        unsafe {
            let dir = PB as *mut u32;
            write_volatile(dir, read_volatile(dir) & !pin);
        }
    }

    fn read_pin(&self, pin: u32) -> bool {
        unsafe { read_volatile((PB + INPUT) as *const u32) & pin != 0 }
    }

    /// Waits until half a bit time has passed since the previous edge.
    fn wait(&mut self) {
        while since(self.edge) < self.half_bit {}
        self.edge = cycles();
    }
}

fn cycles() -> u32 {
    unsafe { read_volatile((TMR0 + 8) as *const u32) }
}

fn since(start: u32) -> u32 {
    cycles().wrapping_sub(start) & (TIMER_WRAP - 1)
}
