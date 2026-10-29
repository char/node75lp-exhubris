use crate::{Packet, Protocol};

pub(super) const FIRST_USAGE: u8 = 0x04;
pub(super) const LAST_USAGE: u8 = 0x73;
const KEY_BITS: usize = (LAST_USAGE - FIRST_USAGE + 1) as usize;

/// Modifier bits and then per-usage bitmap
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report([u8; Self::LEN]);

impl Report {
    pub const LEN: usize = 1 + KEY_BITS.div_ceil(8);

    pub fn new(modifiers: u8, usages: impl IntoIterator<Item = u8>) -> Self {
        let mut report = Self::default();
        report.0[0] = modifiers;
        for usage in usages {
            if (FIRST_USAGE..=LAST_USAGE).contains(&usage) {
                let bit = usize::from(usage - FIRST_USAGE);
                report.0[1 + bit / 8] |= 1 << (bit % 8);
            }
        }
        report
    }

    pub fn packet(self, protocol: Protocol) -> Packet {
        match protocol {
            Protocol::Report => Packet::new(&self.0),
            Protocol::Boot => {
                let mut boot = [0; 8];
                boot[0] = self.0[0];
                let mut slot = 2;
                for bit in 0..KEY_BITS {
                    if self.0[1 + bit / 8] & (1 << (bit % 8)) == 0 {
                        continue;
                    }
                    if slot == boot.len() {
                        boot[2..].fill(1); // HID ErrorRollOver, not arbitrary dropped keys.
                        break;
                    }
                    boot[slot] = FIRST_USAGE + bit as u8;
                    slot += 1;
                }
                Packet::new(&boot)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_preserves_every_supported_key_and_modifiers() {
        let report = Report::new(0xa5, 0x04..=0x73);
        assert_eq!(
            report.packet(Protocol::Report).as_slice(),
            [&[0xa5][..], &[0xff; 14]].concat()
        );
        assert_eq!(
            Report::default().packet(Protocol::Report).as_slice(),
            [0; 15]
        );
    }

    #[test]
    fn bitmap_orders_bits_by_usage_and_deduplicates_keys() {
        let report = Report::new(2, [0x04, 0x0b, 0x0c, 0x73, 0x04, 0, 3, 0x74, 0xff]);
        assert_eq!(
            report.packet(Protocol::Report).as_slice(),
            [2, 0x81, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );
        assert_eq!(
            report.packet(Protocol::Boot).as_slice(),
            [2, 0, 4, 0x0b, 0x0c, 0x73, 0, 0]
        );
    }

    #[test]
    fn boot_protocol_has_six_slots_and_signals_rollover() {
        let six = Report::new(2, 4..=9);
        let seven = Report::new(2, 4..=10);
        assert_eq!(
            six.packet(Protocol::Boot).as_slice(),
            [2, 0, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(
            seven.packet(Protocol::Boot).as_slice(),
            [2, 0, 1, 1, 1, 1, 1, 1]
        );
        assert_eq!(
            Report::new(0x1d, []).packet(Protocol::Boot).as_slice(),
            [0x1d, 0, 0, 0, 0, 0, 0, 0]
        );
    }
}
