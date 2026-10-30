use core::{
    arch::asm,
    ptr::{addr_of_mut, read_volatile, write_volatile},
};

use node75_board_api::{GpioWrite, Node75Board, Register};
use node75_lighting_effects::{FRAME_BYTES, LEDS, Rgb, encode};

use crate::board::Clock;

const SPI: usize = 0x4000_4000;
const DATA_PIN: u32 = 1 << 14;
const POWER_PIN: u32 = 1 << 15;
const FRAME_MS: u32 = 8;

#[repr(C, align(4))]
struct Frame([u8; FRAME_BYTES]);

static mut FRAME: Frame = Frame([0; FRAME_BYTES]);

#[derive(Clone, Copy)]
enum State {
    Off,
    Powering(u32),
    Sending(u32),
    Ready(u32),
    Fault,
}

pub struct Lights {
    gpio: Node75Board,
    state: State,
    supported_clock: bool,
    shown: [Rgb; LEDS],
}

impl Lights {
    /// Requires exclusive SPI0 and DMA-buffer ownership.
    pub unsafe fn new(clock: &Clock, gpio: Node75Board) -> Self {
        let mut lights = Self {
            gpio,
            state: State::Off,
            // Stock's divisor and pulse encoding are characterized only at 60 MHz.
            supported_clock: clock.frequency_hz() == 60_000_000,
            shown: [Rgb::default(); LEDS],
        };
        unsafe { lights.off() };
        lights
    }

    pub fn poll(&mut self, enabled: bool, now: u32, render: impl FnOnce() -> [Rgb; LEDS]) {
        if !enabled {
            if !matches!(self.state, State::Off) {
                unsafe { self.off() };
                self.state = State::Off;
            }
            return;
        }
        if !self.supported_clock {
            return;
        }
        unsafe {
            let force = match self.state {
                State::Off => {
                    // Stock enables lighting by driving PB15 low; off releases it.
                    self.gpio
                        .apply_writes(&[
                            GpioWrite::new(Register::ClearB, 0, POWER_PIN | DATA_PIN),
                            GpioWrite::new(Register::PullDownB, POWER_PIN | DATA_PIN, 0),
                            GpioWrite::new(Register::DirectionB, 0, POWER_PIN | DATA_PIN),
                        ])
                        .expect("lighting GPIO");
                    self.state = State::Powering(userlib::sys_get_timer().now as u32);
                    return;
                }
                State::Powering(since) if now.wrapping_sub(since) >= 2 => {
                    self.gpio
                        .apply_writes(&[GpioWrite::new(Register::PinAlternate, 0, 0x100)])
                        .expect("lighting pin mux");
                    write_volatile((SPI + 3) as *mut u8, 7);
                    write_volatile(SPI as *mut u8, 2);
                    write_volatile(SPI as *mut u8, 0x40);
                    write_volatile((SPI + 1) as *mut u8, 0x10);
                    true
                }
                State::Sending(since) if read_volatile((SPI + 6) as *const u8) & 1 != 0 => {
                    // Count-end, not DMA-end: the FIFO must finish shifting too.
                    write_volatile((SPI + 1) as *mut u8, 0x10);
                    self.state = State::Ready(since);
                    return;
                }
                State::Sending(since) if now.wrapping_sub(since) >= 20 => {
                    self.off();
                    self.state = State::Fault;
                    return;
                }
                State::Ready(since) if now.wrapping_sub(since) >= FRAME_MS => false,
                _ => return,
            };
            let colors = render();
            if !force && colors == self.shown {
                self.state = State::Ready(now);
                return;
            }
            encode(&colors, &mut (*addr_of_mut!(FRAME)).0);
            let start = addr_of_mut!(FRAME) as usize;
            write_volatile((SPI + 0x18) as *mut u16, start as u16);
            write_volatile((SPI + 0x1c) as *mut u16, (start + FRAME_BYTES) as u16);
            write_volatile((SPI + 0x0c) as *mut u16, FRAME_BYTES as u16);
            write_volatile((SPI + 6) as *mut u8, 9);
            asm!("fence iorw, iorw", options(nostack));
            write_volatile((SPI + 1) as *mut u8, 0x11);
            self.shown = colors;
            self.state = State::Sending(now);
        }
    }

    unsafe fn off(&mut self) {
        unsafe {
            write_volatile((SPI + 1) as *mut u8, 0);
            write_volatile((SPI + 2) as *mut u8, 0);
            write_volatile(SPI as *mut u8, 2);
            write_volatile(SPI as *mut u8, 0);
            self.gpio
                .apply_writes(&[
                    GpioWrite::new(Register::ClearB, 0, DATA_PIN),
                    GpioWrite::new(Register::PullUpB, POWER_PIN, 0),
                    GpioWrite::new(Register::PullDownB, POWER_PIN, 0),
                    GpioWrite::new(Register::DirectionB, POWER_PIN, 0),
                ])
                .expect("lighting power off");
        }
    }
}
