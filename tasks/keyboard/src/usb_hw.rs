use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

use crate::board::Clock;

const USB: usize = 0x4000_8000;
const PIN_ANALOG_IE: *mut u16 = 0x4000_101a as *mut u16;
const USB_CTRL: *mut u8 = USB as *mut u8;
const UDEV_CTRL: *mut u8 = (USB + 1) as *mut u8;
const USB_INT_EN: *mut u8 = (USB + 2) as *mut u8;
const USB_DEV_AD: *mut u8 = (USB + 3) as *mut u8;
const USB_MIS_ST: *mut u8 = (USB + 5) as *mut u8;
const USB_INT_FG: *mut u8 = (USB + 6) as *mut u8;
const USB_INT_ST: *mut u8 = (USB + 7) as *mut u8;
const USB_RX_LEN: *mut u8 = (USB + 8) as *mut u8;
const UEP4_1_MOD: *mut u8 = (USB + 0x0c) as *mut u8;
const UEP2_3_MOD: *mut u8 = (USB + 0x0d) as *mut u8;
const UEP567_MOD: *mut u8 = (USB + 0x0e) as *mut u8;
const UEP0_DMA: *mut u16 = (USB + 0x10) as *mut u16;
const UEP1_DMA: *mut u16 = (USB + 0x14) as *mut u16;
const UEP2_DMA: *mut u16 = (USB + 0x18) as *mut u16;
const UEP0_T_LEN: *mut u8 = (USB + 0x20) as *mut u8;
const UEP0_CTRL: *mut u8 = (USB + 0x22) as *mut u8;
const UEP1_T_LEN: *mut u8 = (USB + 0x24) as *mut u8;
const UEP1_CTRL: *mut u8 = (USB + 0x26) as *mut u8;
const UEP2_T_LEN: *mut u8 = (USB + 0x28) as *mut u8;
const UEP2_CTRL: *mut u8 = (USB + 0x2a) as *mut u8;

const UC_DEV_PU_EN: u8 = 0x20;
const UC_INT_BUSY: u8 = 0x08;
const UC_DMA_EN: u8 = 0x01;
const UC_RESET_SIE: u8 = 0x04;
const UC_CLR_ALL: u8 = 0x02;
const UD_PD_DIS: u8 = 0x80;
const UD_PORT_EN: u8 = 0x01;
const UIE_SUSPEND: u8 = 0x04;
const UIE_TRANSFER: u8 = 0x02;
const UIE_BUS_RST: u8 = 0x01;
const UIF_SUSPEND: u8 = 0x04;
const UIF_TRANSFER: u8 = 0x02;
const UIF_BUS_RST: u8 = 0x01;
const UIS_SETUP_ACT: u8 = 0x80;
const UIS_TOG_OK: u8 = 0x40;
const UIS_TOKEN: u8 = 0x30;
const UIS_ENDP: u8 = 0x0f;
const TOKEN_OUT: u8 = 0x00;
const TOKEN_IN: u8 = 0x20;
const TOKEN_IDLE: u8 = 0x30;
const UEP_R_TOG: u8 = 0x80;
const UEP_T_TOG: u8 = 0x40;
const UEP_R_RES: u8 = 0x0c;
const UEP_T_RES: u8 = 0x03;
const UEP_R_ACK: u8 = 0x00;
const UEP_R_STALL: u8 = 0x0c;
const UEP_T_ACK: u8 = 0x00;
const UEP_T_NAK: u8 = 0x02;
const UEP_T_STALL: u8 = 0x03;
const UEP_AUTO_TOG: u8 = 0x10;
const EP0_MAX: usize = 64;
const EP1_MAX: usize = 8;
const EP2_MAX: usize = 2;
const UIF_EVENTS: u8 = UIF_BUS_RST | UIF_TRANSFER | UIF_SUSPEND;

#[repr(align(4))]
struct DmaBuf([u8; 64]);

static mut EP0_BUF: DmaBuf = DmaBuf([0; 64]);
static mut EP1_BUF: DmaBuf = DmaBuf([0; 64]);
static mut EP2_BUF: DmaBuf = DmaBuf([0; 64]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Reset,
    Setup([u8; 8]),
    Out {
        endpoint: u8,
        data: [u8; 64],
        len: usize,
    },
    InComplete(u8),
    Suspend(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ep0Response {
    None,
    Setup,
    Completion,
}

pub struct Usb {
    configured: bool,
    ep0_busy: bool,
    ep1_busy: bool,
    ep2_busy: bool,
    ep1_halted: bool,
    ep2_halted: bool,
    ep0_next_toggle: bool,
    ep1_next_toggle: bool,
    ep2_next_toggle: bool,
    suspend_state: Option<bool>,
    held_flags: u8,
    ep0_response: Ep0Response,
}

impl Usb {
    /// Call once, after the guard, with exclusive USB access and interrupts disabled.
    pub unsafe fn new(clock: &Clock) -> Self {
        unsafe {
            // Disconnect first so a warm loader-to-application handoff is observable.
            write_volatile(USB_CTRL, 0);
            let analog = read_volatile(PIN_ANALOG_IE);
            write_volatile(PIN_ANALOG_IE, (analog | 0x80) & !0x40);
            write_volatile(UDEV_CTRL, 0);
            write_volatile(USB_INT_EN, 0);
            write_volatile(UEP4_1_MOD, 0);
            write_volatile(UEP2_3_MOD, 0);
            write_volatile(UEP567_MOD, 0);
            write_volatile(USB_CTRL, UC_RESET_SIE | UC_CLR_ALL);
            write_volatile(USB_CTRL, 0);
            write_volatile(
                UEP0_DMA,
                dma_addr(core::ptr::addr_of_mut!(EP0_BUF.0).cast::<u8>()),
            );
            write_volatile(
                UEP1_DMA,
                dma_addr(core::ptr::addr_of_mut!(EP1_BUF.0).cast::<u8>()),
            );
            write_volatile(
                UEP2_DMA,
                dma_addr(core::ptr::addr_of_mut!(EP2_BUF.0).cast::<u8>()),
            );
            write_volatile(USB_DEV_AD, 0);
            write_volatile(UEP0_CTRL, UEP_R_ACK | UEP_T_NAK);
            write_volatile(UEP1_CTRL, UEP_R_ACK | UEP_T_NAK);
            write_volatile(UEP2_CTRL, UEP_R_ACK | UEP_T_NAK);
            write_volatile(UEP4_1_MOD, 0x40); // EP1 IN only, one 64-byte buffer.
            write_volatile(UEP2_3_MOD, 0x04); // EP2 IN only, one 64-byte buffer.
            write_volatile(USB_INT_FG, 0xff);
            write_volatile(USB_CTRL, UC_INT_BUSY | UC_DMA_EN);
            let disconnected_at = clock.millis();
            while clock.millis().wrapping_sub(disconnected_at) < 100 {
                core::hint::spin_loop();
            }
            write_volatile(UDEV_CTRL, UD_PD_DIS | UD_PORT_EN);
            write_volatile(USB_CTRL, UC_DEV_PU_EN | UC_INT_BUSY | UC_DMA_EN);
            write_volatile(USB_INT_EN, UIE_SUSPEND | UIE_BUS_RST | UIE_TRANSFER);
        }
        Self {
            configured: false,
            ep0_busy: false,
            ep1_busy: false,
            ep2_busy: false,
            ep1_halted: false,
            ep2_halted: false,
            ep0_next_toggle: true,
            ep1_next_toggle: false,
            ep2_next_toggle: false,
            suspend_state: None,
            held_flags: 0,
            ep0_response: Ep0Response::None,
        }
    }

    pub fn poll(&mut self) -> Option<Event> {
        unsafe {
            let flags = read_volatile(USB_INT_FG);
            if self.held_flags != 0 && flags & UIF_BUS_RST == 0 {
                return None;
            }
            if flags & UIF_BUS_RST != 0 {
                write_volatile(USB_DEV_AD, 0);
                write_volatile(UEP0_T_LEN, 0);
                write_volatile(UEP1_T_LEN, 0);
                write_volatile(UEP2_T_LEN, 0);
                write_volatile(UEP0_CTRL, UEP_R_ACK | UEP_T_NAK);
                write_volatile(UEP1_CTRL, UEP_R_ACK | UEP_T_NAK);
                write_volatile(UEP2_CTRL, UEP_R_ACK | UEP_T_NAK);
                self.configured = false;
                self.ep0_busy = false;
                self.ep1_busy = false;
                self.ep2_busy = false;
                self.ep1_halted = false;
                self.ep2_halted = false;
                self.ep0_next_toggle = true;
                self.ep1_next_toggle = false;
                self.ep2_next_toggle = false;
                self.suspend_state = None;
                self.held_flags = flags & UIF_EVENTS;
                self.ep0_response = Ep0Response::None;
                return Some(Event::Reset);
            }

            if flags & UIF_TRANSFER != 0 {
                let status = read_volatile(USB_INT_ST);
                let token = status & UIS_TOKEN;
                let endpoint = status & UIS_ENDP;
                // A stacked SETUP needs a separate W1C after retiring the old token
                // (CH583DS1 V1.9 p122); otherwise its DMA bytes are processed twice.
                if token == TOKEN_IDLE && status & UIS_SETUP_ACT != 0 {
                    return Some(self.setup_event());
                }
                match (endpoint, token) {
                    (_, TOKEN_IN) => {
                        if let Some(event) = self.complete_in(endpoint) {
                            self.held_flags = UIF_TRANSFER;
                            return Some(event);
                        }
                    }
                    (0, TOKEN_OUT) if status & UIS_TOG_OK != 0 => {
                        let len = read_volatile(USB_RX_LEN) as usize;
                        if len <= 64 {
                            let source = core::ptr::addr_of!(EP0_BUF.0).cast::<u8>();
                            let mut data = [0; 64];
                            for (i, byte) in data.iter_mut().take(len).enumerate() {
                                *byte = read_volatile(source.add(i));
                            }
                            let ctrl = read_volatile(UEP0_CTRL);
                            write_volatile(UEP0_CTRL, ctrl ^ UEP_R_TOG);
                            self.ep0_response = Ep0Response::Completion;
                            self.held_flags = UIF_TRANSFER;
                            return Some(Event::Out {
                                endpoint,
                                data,
                                len,
                            });
                        }
                    }
                    _ => {}
                }
                write_volatile(USB_INT_FG, UIF_TRANSFER);
            }

            if flags & UIF_SUSPEND != 0 {
                let suspended = read_volatile(USB_MIS_ST) & 0x04 != 0;
                if self.suspend_state != Some(suspended) {
                    if suspended {
                        // Pending completions were retired above. Cancel unpolled
                        // input without advancing either endpoint's data toggle.
                        for (busy, control, length) in [
                            (&mut self.ep1_busy, UEP1_CTRL, UEP1_T_LEN),
                            (&mut self.ep2_busy, UEP2_CTRL, UEP2_T_LEN),
                        ] {
                            if *busy {
                                let ctrl = read_volatile(control);
                                write_volatile(control, (ctrl & !UEP_T_RES) | UEP_T_NAK);
                                write_volatile(length, 0);
                                *busy = false;
                            }
                        }
                    }
                    self.suspend_state = Some(suspended);
                    self.held_flags = UIF_SUSPEND;
                    return Some(Event::Suspend(suspended));
                }
                write_volatile(USB_INT_FG, UIF_SUSPEND);
            }
        }
        None
    }

    pub fn finish_event(&mut self) {
        if self.held_flags == 0 {
            return;
        }
        unsafe {
            write_volatile(USB_INT_FG, self.held_flags);
        }
        self.held_flags = 0;
        self.ep0_response = Ep0Response::None;
    }

    fn complete_in(&mut self, endpoint: u8) -> Option<Event> {
        let (busy, toggle, control) = match endpoint {
            0 => (&mut self.ep0_busy, &mut self.ep0_next_toggle, UEP0_CTRL),
            1 => (&mut self.ep1_busy, &mut self.ep1_next_toggle, UEP1_CTRL),
            2 => (&mut self.ep2_busy, &mut self.ep2_next_toggle, UEP2_CTRL),
            _ => return None,
        };
        if !*busy {
            return None;
        }
        *busy = false;
        *toggle = !*toggle;
        unsafe {
            let ctrl = read_volatile(control);
            write_volatile(control, (ctrl & !UEP_T_RES) | UEP_T_NAK);
        }
        if endpoint == 0 {
            self.ep0_response = Ep0Response::Completion;
        }
        Some(Event::InComplete(endpoint))
    }

    fn setup_event(&mut self) -> Event {
        unsafe {
            let data = core::ptr::addr_of!(EP0_BUF.0).cast::<u8>();
            let setup = [
                read_volatile(data),
                read_volatile(data.add(1)),
                read_volatile(data.add(2)),
                read_volatile(data.add(3)),
                read_volatile(data.add(4)),
                read_volatile(data.add(5)),
                read_volatile(data.add(6)),
                read_volatile(data.add(7)),
            ];
            self.ep0_busy = false;
            self.ep0_next_toggle = true;
            self.ep0_response = Ep0Response::Setup;
            write_volatile(UEP0_T_LEN, 0);
            write_volatile(UEP0_CTRL, UEP_R_TOG | UEP_T_TOG | UEP_R_ACK | UEP_T_NAK);
            self.held_flags = UIF_TRANSFER;
            Event::Setup(setup)
        }
    }

    pub fn transmit(&mut self, endpoint: u8, data: &[u8]) -> bool {
        let held_ep0_response = endpoint == 0
            && self.held_flags == UIF_TRANSFER
            && self.ep0_response != Ep0Response::None;
        let pending = unsafe { read_volatile(USB_INT_FG) } & UIF_EVENTS;
        if self.held_flags != 0 {
            if !held_ep0_response || pending & UIF_BUS_RST != 0 {
                return false;
            }
            // A completed token may leave a new SETUP latched in the same DMA buffer.
            if self.ep0_response != Ep0Response::Setup
                && unsafe { read_volatile(USB_INT_ST) } & UIS_SETUP_ACT != 0
            {
                return false;
            }
        } else if pending != 0 {
            return false;
        }
        let (capacity, dma, length, control, busy, toggle) = match endpoint {
            0 => (
                EP0_MAX,
                unsafe { core::ptr::addr_of_mut!(EP0_BUF.0).cast::<u8>() },
                UEP0_T_LEN,
                UEP0_CTRL,
                &mut self.ep0_busy,
                self.ep0_next_toggle,
            ),
            1 => {
                if !self.configured || self.ep1_halted {
                    return false;
                }
                (
                    EP1_MAX,
                    unsafe { core::ptr::addr_of_mut!(EP1_BUF.0).cast::<u8>() },
                    UEP1_T_LEN,
                    UEP1_CTRL,
                    &mut self.ep1_busy,
                    self.ep1_next_toggle,
                )
            }
            2 => {
                if !self.configured || self.ep2_halted {
                    return false;
                }
                (
                    EP2_MAX,
                    unsafe { core::ptr::addr_of_mut!(EP2_BUF.0).cast::<u8>() },
                    UEP2_T_LEN,
                    UEP2_CTRL,
                    &mut self.ep2_busy,
                    self.ep2_next_toggle,
                )
            }
            _ => return false,
        };
        if *busy || data.len() > capacity {
            return false;
        }
        unsafe {
            for (i, byte) in data.iter().copied().enumerate() {
                write_volatile(dma.add(i), byte);
            }
            asm!("fence iorw, iorw", options(nostack));
            write_volatile(length, data.len() as u8);
            let mut ctrl = read_volatile(control) & !(UEP_T_RES | UEP_T_TOG);
            if toggle {
                ctrl |= UEP_T_TOG;
            }
            write_volatile(control, ctrl | UEP_T_ACK);
        }
        *busy = true;
        true
    }

    pub fn set_address(&mut self, address: u8) {
        unsafe {
            write_volatile(USB_DEV_AD, address & 0x7f);
        }
    }

    pub fn configure(&mut self, configured: bool) {
        self.configured = configured;
        self.ep1_busy = false;
        self.ep2_busy = false;
        self.ep1_next_toggle = false;
        self.ep2_next_toggle = false;
        self.ep1_halted = false;
        self.ep2_halted = false;
        unsafe {
            write_volatile(UEP1_CTRL, UEP_R_ACK | UEP_T_NAK);
            write_volatile(UEP2_CTRL, UEP_R_ACK | UEP_T_NAK);
        }
    }

    pub fn stall_control(&mut self) {
        self.ep0_busy = false;
        unsafe {
            write_volatile(UEP0_CTRL, UEP_R_STALL | UEP_T_STALL);
        }
    }

    pub fn set_halt(&mut self, endpoint: u8, halted: bool) {
        let (busy, halt, toggle, control) = match endpoint {
            0x81 => (
                &mut self.ep1_busy,
                &mut self.ep1_halted,
                &mut self.ep1_next_toggle,
                UEP1_CTRL,
            ),
            0x82 => (
                &mut self.ep2_busy,
                &mut self.ep2_halted,
                &mut self.ep2_next_toggle,
                UEP2_CTRL,
            ),
            _ => return,
        };
        *busy = false;
        *halt = halted;
        *toggle = false;
        unsafe {
            let mut ctrl = read_volatile(control)
                & !(UEP_R_RES | UEP_T_RES | UEP_R_TOG | UEP_T_TOG | UEP_AUTO_TOG);
            ctrl |= if halted {
                UEP_R_STALL | UEP_T_STALL
            } else {
                UEP_R_ACK | UEP_T_NAK
            };
            write_volatile(control, ctrl);
        }
    }
}

fn dma_addr(ptr: *mut u8) -> u16 {
    ptr as usize as u16
}
