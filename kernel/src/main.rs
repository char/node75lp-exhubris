#![no_std]
#![no_main]

use core::{
    arch::global_asm,
    ptr::{addr_of_mut, write_volatile},
};

use node75_matrix as matrix;

mod board;
mod recovery;

// The pre-kernel boot guard: samples the physical matrix for Space and, if
// held, requests the vendor loader via the RAM-resident recovery path. It
// runs before start_kernel, before U-mode, and before any PMP setup.
include!("boot.rs");

#[unsafe(no_mangle)]
#[inline(never)]
extern "C" fn application() -> ! {
    unsafe {
        // Disconnect inherited loader state before handing USB ownership to a task.
        write_volatile(0x4000_8000 as *mut u8, 0);
        let Some(clock) = board::initialize() else {
            set_state(BootState::UsbClockUnavailable);
            panic!("unsupported inherited clock");
        };
        hubris_kern::startup::start_kernel(clock.frequency_hz() / 1_000)
    }
}
