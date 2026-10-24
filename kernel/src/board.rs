use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

const SYSTICK: usize = 0xe000_f000;

pub struct Clock {
    cycles_per_ms: u64,
}

impl Clock {
    pub fn frequency_hz(&self) -> u32 {
        (self.cycles_per_ms * 1000) as u32
    }
}

unsafe fn protected_write(address: usize, value: u8) {
    unsafe {
        write_volatile(0x4000_1040 as *mut u8, 0x57);
        write_volatile(0x4000_1040 as *mut u8, 0xa8);
        // The RAM flash driver demonstrated that immediate protected writes can fail.
        asm!("nop", "nop", options(nostack));
        write_volatile(address as *mut u8, value);
        write_volatile(0x4000_1040 as *mut u8, 0);
    }
}

/// Application-only handoff; the recovery path does not depend on this setup.
/// Requires exclusive peripheral access with interrupts disabled.
pub unsafe fn initialize() -> Option<Clock> {
    unsafe {
        // No inherited watchdog policy in this polled application.
        protected_write(0x4000_1046, 0x10);
        let config = read_volatile(0x4000_1008 as *const u16);
        let divisor = u32::from(config & 0x1f);
        let frequency = match config & 0xc0 {
            0x00 if divisor != 0 => 32_000_000 / divisor,
            0x40 if divisor != 0 => 480_000_000 / divisor,
            0x80 => 32_000_000,
            _ => return None,
        };
        // USB needs the inherited 480 MHz PLL. Do not switch CPU/flash clocks here.
        if !(2_000_000..=80_000_000).contains(&frequency)
            || read_volatile(0x4000_100a as *const u8) & 0x14 != 0x14
        {
            return None;
        }
        protected_write(0x4000_100c, 0);
        protected_write(0x4000_100d, read_volatile(0x4000_100d as *const u8) & !0x1f);

        // Reclaim matrix, USB, and touch pins from inherited peripheral overrides.
        for timer in [0x4000_2000usize, 0x4000_2400, 0x4000_2800, 0x4000_2c00] {
            write_volatile(timer as *mut u8, 0);
        }
        for timer_dma in [0x4000_2401usize, 0x4000_2801] {
            write_volatile(timer_dma as *mut u8, 0);
        }
        for uart in [0x4000_3001usize, 0x4000_3401, 0x4000_3801, 0x4000_3c01] {
            write_volatile(uart as *mut u8, 0x80);
            write_volatile(uart as *mut u8, 0);
        }
        for spi in [0x4000_4000usize, 0x4000_4400] {
            write_volatile((spi + 1) as *mut u8, 0);
            write_volatile(spi as *mut u8, 0);
        }
        // The touch task bit-bangs PB20/PB21 rather than using hardware I2C.
        write_volatile(0x4000_4800 as *mut u16, 0x8000);
        write_volatile(0x4000_4800 as *mut u16, 0);
        write_volatile(0x4000_4804 as *mut u16, 0);
        write_volatile(0x4000_5000 as *mut u8, 0); // PWM4-11 outputs off
        write_volatile(0x4000_1057 as *mut u8, 0); // TouchKey module off
        write_volatile(0x4000_1060 as *mut u8, 0);
        write_volatile(0x4000_1059 as *mut u8, 0);
        let alternate = 0x4000_1018 as *mut u16;
        write_volatile(alternate, read_volatile(alternate) & !0x8000);
        let analog = 0x4000_101a as *mut u16;
        write_volatile(analog, read_volatile(analog) & !0xd003);

        // PB10/PB11 are USB D-/D+, not matrix pins; remove inherited GPIO pulls/drivers.
        for address in [0x4000_10c0usize, 0x4000_10d0, 0x4000_10d4] {
            let register = address as *mut u32;
            write_volatile(register, read_volatile(register) & !0xc00);
        }

        write_volatile(SYSTICK as *mut u32, 0);
        write_volatile((SYSTICK + 8) as *mut u32, 0);
        write_volatile((SYSTICK + 12) as *mut u32, 0);
        write_volatile((SYSTICK + 16) as *mut u32, u32::MAX);
        write_volatile((SYSTICK + 20) as *mut u32, u32::MAX);
        write_volatile((SYSTICK + 4) as *mut u32, 0);
        write_volatile(SYSTICK as *mut u32, 0x05); // HCLK, free-running, no interrupt.
        Some(Clock {
            cycles_per_ms: u64::from(frequency / 1000),
        })
    }
}
