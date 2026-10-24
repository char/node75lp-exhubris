global_asm!(include_str!("start.S"));

#[unsafe(no_mangle)]
static mut BOOT_MATRIX: [u16; 6] = [0; 6];
#[repr(u32)]
#[derive(Clone, Copy)]
enum BootState {
    Sampling = 1,
    Application = 2,
    RequestingUpdater = 3,
    Resetting = 4,
    UsbClockUnavailable = 5,
    AcknowledgingApplication = 6,
    ControllerTimeout = 0x81,
    FlashTimeout = 0x82,
    VerifyFailed = 0x83,
    ResetReturned = 0x84,
}
#[unsafe(no_mangle)]
static mut BOOT_STATE: u32 = 0;
#[inline(always)]
unsafe fn set_state(state: BootState) {
    unsafe { write_volatile(addr_of_mut!(BOOT_STATE), state as u32) };
}

#[unsafe(no_mangle)]
extern "C" fn rust_entry() -> ! {
    unsafe {
        set_state(BootState::Sampling);
        let mut space_held = true;
        for _ in 0..16 {
            let sample = matrix::sample();
            write_volatile(addr_of_mut!(BOOT_MATRIX), sample);
            space_held &= sample[5] & (1 << 5) != 0;
        }
        if space_held {
            set_state(BootState::RequestingUpdater);
            recovery::enter_updater();
        }
        set_state(BootState::AcknowledgingApplication);
        recovery::acknowledge_application();
        set_state(BootState::Application);
    }
    application()
}

