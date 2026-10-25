//! Open-drain I2C on PB20/PB21. No syscalls inside a transaction: GPIO
//! read-modify-writes cannot race the other cooperative tasks.

use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

const PB: usize = 0x4000_10c0;
const INPUT: usize = 4;
const CLEAR: usize = 12;
const PULL_UP: usize = 16;
const PULL_DOWN: usize = 20;
const SDA: u32 = 1 << 20;
const SCL: u32 = 1 << 21;
// Shared across every clock edge, including STOP; not a per-bit timeout.
const STRETCH_POLLS: u32 = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("touch controller NACK")]
    Nack,
    #[error("touch clock stretch budget exhausted")]
    ClockTimeout,
    #[error("touch SDA stuck low")]
    BusBusy,
}

pub struct I2c {
    address: u8,
    stretch_left: u32,
}

impl I2c {
    /// Requires exclusive access to PB20/PB21.
    pub unsafe fn new(seven_bit_address: u8) -> Self {
        unsafe {
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
            stretch_left: 0,
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
        self.stretch_left = STRETCH_POLLS;
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
        Self::delay();
        self.release_clock()?;
        Self::delay();
        if !self.read_pin(SDA) {
            return Err(Error::BusBusy);
        }
        self.drive_low(SDA);
        Self::delay();
        self.drive_low(SCL);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.drive_low(SCL);
        self.drive_low(SDA);
        Self::delay();
        let result = self.release_clock();
        Self::delay();
        self.release(SDA);
        Self::delay();
        result
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), Error> {
        for bit in (0..8).rev() {
            if byte & (1 << bit) != 0 {
                self.release(SDA);
            } else {
                self.drive_low(SDA);
            }
            Self::delay();
            self.release_clock()?;
            Self::delay();
            self.drive_low(SCL);
        }
        self.release(SDA);
        Self::delay();
        self.release_clock()?;
        Self::delay();
        let ack = !self.read_pin(SDA);
        self.drive_low(SCL);
        if ack { Ok(()) } else { Err(Error::Nack) }
    }

    fn read_byte(&mut self, ack: bool) -> Result<u8, Error> {
        self.release(SDA);
        let mut byte = 0u8;
        for _ in 0..8 {
            Self::delay();
            self.release_clock()?;
            Self::delay();
            byte = (byte << 1) | u8::from(self.read_pin(SDA));
            self.drive_low(SCL);
        }
        if ack {
            self.drive_low(SDA);
        }
        Self::delay();
        self.release_clock()?;
        Self::delay();
        self.drive_low(SCL);
        self.release(SDA);
        Ok(byte)
    }

    fn release_clock(&mut self) -> Result<(), Error> {
        self.release(SCL);
        while !self.read_pin(SCL) {
            if self.stretch_left == 0 {
                return Err(Error::ClockTimeout);
            }
            self.stretch_left -= 1;
        }
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

    fn delay() {
        // Conservative at 60 MHz; verify SCL timing on the board.
        for _ in 0..100 {
            unsafe { asm!("nop", options(nomem, nostack)) };
        }
    }
}
