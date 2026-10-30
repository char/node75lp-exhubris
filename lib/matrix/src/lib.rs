//! CH582 GPIO matrix scan for the Node75 LP (6 drive rows × 16 sense
//! columns). Used by the pre-kernel boot guard and the board-I/O task.
//! Pin assignments recovered from the stock firmware.

#![no_std]

use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

#[derive(Clone, Copy)]
enum Pin {
    A(u8),
    B(u8),
}

const SENSE: [Pin; 16] = [
    Pin::A(0),
    Pin::A(1),
    Pin::A(2),
    Pin::A(3),
    Pin::A(15),
    Pin::A(14),
    Pin::A(13),
    Pin::B(6),
    Pin::A(9),
    Pin::A(12),
    Pin::B(5),
    Pin::B(4),
    Pin::B(3),
    Pin::B(2),
    Pin::B(1),
    Pin::B(0),
];
const DRIVE: [u8; 6] = [9, 8, 17, 16, 19, 18];

const PA: usize = 0x4000_10a0;
const PB: usize = 0x4000_10c0;
const DIR: usize = 0;
const INPUT: usize = 4;
const CLEAR: usize = 12;
const PULL_UP: usize = 16;
const PULL_DOWN_DRIVE: usize = 20;

/// Requires exclusive GPIO-register ownership, or pre-kernel execution with interrupts disabled.
pub unsafe fn sample() -> [u16; 6] {
    unsafe {
        for (port, mask) in [(PA, 0xf20f), (PB, 0x000f_037f)] {
            let dir = (port + DIR) as *mut u32;
            let pull_up = (port + PULL_UP) as *mut u32;
            let drive = (port + PULL_DOWN_DRIVE) as *mut u32;
            write_volatile(dir, read_volatile(dir) & !mask);
            write_volatile(drive, read_volatile(drive) & !mask);
            write_volatile(pull_up, read_volatile(pull_up) | mask);
        }

        let mut result = [0; 6];
        for (row, pin) in DRIVE.iter().enumerate() {
            let mask = 1 << pin;
            let dir = (PB + DIR) as *mut u32;
            // Latch low before enabling output to avoid a high-going glitch.
            write_volatile((PB + CLEAR) as *mut u32, mask);
            write_volatile(dir, read_volatile(dir) | mask);
            // Conservative settling interval; the inherited clock is unknown.
            for _ in 0..1024 {
                asm!("nop", options(nomem, nostack));
            }
            let a = read_volatile((PA + INPUT) as *const u32);
            let b = read_volatile((PB + INPUT) as *const u32);
            for (bit, pin) in SENSE.iter().enumerate() {
                let high = match pin {
                    Pin::A(pin) => a & (1 << pin),
                    Pin::B(pin) => b & (1 << pin),
                };
                if high == 0 {
                    result[row] |= 1 << bit;
                }
            }
            write_volatile(dir, read_volatile(dir) & !mask);
        }
        result
    }
}
