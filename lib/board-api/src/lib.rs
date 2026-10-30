#![no_std]

pub const MAX_I2C_BYTES: usize = 32;
pub const MAX_WRITES: usize = 16;
pub const LIGHTING_PINS: u32 = (1 << 14) | (1 << 15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Register {
    DirectionB = 0,
    PullUpB = 1,
    PullDownB = 2,
    ClearB = 3,
    PinAlternate = 4,
}

/// A masked GPIO update, not an arbitrary MMIO write.
/// ClearB is write-only: its clear mask must be zero.
#[derive(Clone, Copy, Debug, zerocopy::FromBytes, zerocopy::IntoBytes, zerocopy::Immutable)]
#[repr(C)]
pub struct GpioWrite {
    register: u32,
    pub clear: u32,
    pub set: u32,
}

impl GpioWrite {
    pub const fn new(register: Register, clear: u32, set: u32) -> Self {
        Self {
            register: register as u32,
            clear,
            set,
        }
    }

    pub fn register(self) -> Option<Register> {
        let register = match self.register {
            0 => Register::DirectionB,
            1 => Register::PullUpB,
            2 => Register::PullDownB,
            3 => Register::ClearB,
            4 => Register::PinAlternate,
            _ => return None,
        };
        let allowed = match register {
            Register::PinAlternate => 0x100,
            _ => LIGHTING_PINS,
        };
        if (self.clear | self.set) & !allowed != 0
            || (register == Register::ClearB && self.clear != 0)
        {
            return None;
        }
        Some(register)
    }
}

#[cfg(target_os = "none")]
include!(concat!(env!("OUT_DIR"), "/client.rs"));

#[cfg(test)]
mod generated_contract;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_batches_cannot_change_matrix_i2c_or_usb_pins() {
        for register in [
            Register::DirectionB,
            Register::PullUpB,
            Register::PullDownB,
            Register::ClearB,
        ] {
            assert!(
                GpioWrite::new(register, 0, LIGHTING_PINS)
                    .register()
                    .is_some()
            );
            for pin in 0..32 {
                if LIGHTING_PINS & (1 << pin) == 0 {
                    assert!(GpioWrite::new(register, 0, 1 << pin).register().is_none());
                    assert!(GpioWrite::new(register, 1 << pin, 0).register().is_none());
                }
            }
        }
    }

    #[test]
    fn alternate_function_and_write_only_registers_have_narrow_masks() {
        assert_eq!(
            GpioWrite::new(Register::PinAlternate, 0, 0x100).register(),
            Some(Register::PinAlternate)
        );
        assert!(
            GpioWrite::new(Register::PinAlternate, 0, 0x8000)
                .register()
                .is_none()
        );
        assert!(
            GpioWrite::new(Register::ClearB, 1 << 14, 0)
                .register()
                .is_none()
        );
        assert!(
            GpioWrite {
                register: u32::MAX,
                clear: 0,
                set: 0
            }
            .register()
            .is_none()
        );
    }
}
