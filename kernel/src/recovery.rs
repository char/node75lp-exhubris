use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

use crate::{BootState, set_state};

const DATA: *mut u8 = 0x4000_1804 as *mut u8;
const CONTROL: *mut u8 = 0x4000_1806 as *mut u8;
const ROM_CONFIG: *mut u8 = 0x4000_1044 as *mut u8;
const POLL_LIMIT: u32 = 0x80000;

#[repr(u8)]
enum BootFlag {
    Application = 0xaa,
    Updater = 0x55,
}

// Controller accesses and failure paths stay in RAM; restore flash before returning.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
pub unsafe extern "C" fn acknowledge_application() {
    unsafe { set_flag(BootFlag::Application) };
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
pub unsafe extern "C" fn enter_updater() -> ! {
    unsafe {
        set_flag(BootFlag::Updater);
        begin(false);
        start(0x66);
        finish();
        start(0x99);
        finish();
        end();

        set_state(BootState::Resetting);
        asm!(
            "li t0, 0x40001040",
            "li t1, 0x57",
            "li t2, 0xa8",
            "sb t1, 0(t0)",
            "sb t2, 0(t0)",
            "nop",
            "nop",
            "lbu t1, 6(t0)",
            "ori t1, t1, 1",
            "sb t1, 6(t0)",
            "sb zero, 0(t0)",
            out("t0") _, out("t1") _, out("t2") _,
            options(nostack),
        );
        fail(BootState::ResetReturned)
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn set_flag(flag: BootFlag) {
    unsafe {
        asm!("csrci mstatus, 8", options(nostack));
        write_volatile(0xe000_e180 as *mut u32, u32::MAX);
        write_volatile(0xe000_e184 as *mut u32, u32::MAX);
        asm!("fence.i", options(nostack));

        let original = read_flag_word();
        let requested = (original & !0xff) | flag as u32;
        if requested != original {
            // Match stock: erase 256 bytes, restore only the first four.
            begin(true);
            command_at_flag(0x81, false);
            wait_flash();
            end();

            begin(true);
            command_at_flag(0x02, false);
            for byte in 0..4 {
                send((requested >> (byte * 8)) as u8);
            }
            wait_flash();
            end();

            if read_flag_word() != requested {
                fail(BootState::VerifyFailed);
            }
        }
        asm!("fence.i", options(nostack));
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn read_flag_word() -> u32 {
    unsafe {
        begin(false);
        command_at_flag(0x0b, true);
        let mut word = 0;
        for byte in 0..4 {
            word |= (receive() as u32) << (byte * 8);
        }
        finish();
        end();
        word
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn begin(writable: bool) {
    unsafe {
        configure(read_volatile(ROM_CONFIG) | if writable { 0xe0 } else { 0x20 });
        write_volatile(CONTROL, 4);
        start(0xff);
        finish();
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn end() {
    unsafe { configure(read_volatile(ROM_CONFIG) & 0x10) };
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn configure(value: u8) {
    // Match WCH's SAFEOPERATE gap after unlock, within the safe-access window.
    unsafe {
        asm!(
            "li t0, 0x40001040",
            "li t1, 0x57",
            "li t2, 0xa8",
            "sb t1, 0(t0)",
            "sb t2, 0(t0)",
            "nop",
            "nop",
            "sb {value}, 4(t0)",
            "sb zero, 0(t0)",
            value = in(reg) value as u32,
            out("t0") _, out("t1") _, out("t2") _,
            options(nostack),
        );
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn start(opcode: u8) {
    unsafe {
        write_volatile(CONTROL, 0);
        write_volatile(CONTROL, 5);
        asm!("nop", options(nostack));
        write_volatile(DATA, opcode);
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn command_at_flag(opcode: u8, reading: bool) {
    unsafe {
        if !reading {
            start(0x06);
            finish();
        }
        start(opcode);
        // EEPROM 0 maps to (0x70000 + 0) | 0x80000, not program flash 0.
        send(0x0f);
        send(0);
        send(0);
        if reading {
            // Stock's fast-read path emits two extra zero bytes.
            send(0);
            send(0);
        }
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn wait_controller() {
    unsafe {
        for _ in 0..POLL_LIMIT {
            if read_volatile(CONTROL) & 0x80 == 0 {
                return;
            }
        }
        fail(BootState::ControllerTimeout)
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn finish() {
    unsafe {
        wait_controller();
        write_volatile(CONTROL, 0);
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn send(byte: u8) {
    unsafe {
        wait_controller();
        write_volatile(DATA, byte);
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn receive() -> u8 {
    unsafe {
        wait_controller();
        read_volatile(DATA)
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn wait_flash() {
    unsafe {
        finish();
        for _ in 0..POLL_LIMIT {
            start(0x05);
            let _ = receive();
            let status = receive();
            finish();
            if status & 1 == 0 {
                return;
            }
        }
        fail(BootState::FlashTimeout)
    }
}

#[unsafe(link_section = ".ramtext.recovery")]
#[inline(never)]
unsafe fn fail(state: BootState) -> ! {
    unsafe {
        set_state(state);
        loop {
            asm!("nop", options(nomem, nostack));
        }
    }
}
