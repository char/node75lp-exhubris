//! Wire format for the keyboard→lighting state snapshot.

#![no_std]

pub const GET_STATE: u16 = 1;
pub const STATE_BYTES: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub now: u32,
    pub keys: [u16; 6],
    pub active: bool,
    pub leds: u8,
}

impl State {
    pub fn encode(self) -> [u8; STATE_BYTES] {
        let mut bytes = [0; STATE_BYTES];
        bytes[..4].copy_from_slice(&self.now.to_le_bytes());
        for (key, dest) in self.keys.iter().zip(bytes[4..16].chunks_exact_mut(2)) {
            dest.copy_from_slice(&key.to_le_bytes());
        }
        bytes[16] = u8::from(self.active);
        bytes[17] = self.leds;
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; STATE_BYTES] = bytes.try_into().ok()?;
        let active = match bytes[16] {
            0 => false,
            1 => true,
            _ => return None,
        };
        let mut keys = [0; 6];
        for (key, src) in keys.iter_mut().zip(bytes[4..16].chunks_exact(2)) {
            *key = u16::from_le_bytes([src[0], src[1]]);
        }
        Some(Self {
            now: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            keys,
            active,
            leds: bytes[17],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_wire_vector_preserves_matrix_time_and_host_state() {
        let state = State {
            now: 0x12345678,
            keys: [1, 0x8000, 0xffff, 2, 0, 0x100],
            active: true,
            leds: 2,
        };
        let bytes = [
            0x78, 0x56, 0x34, 0x12, 1, 0, 0, 0x80, 0xff, 0xff, 2, 0, 0, 0, 0, 1, 1, 2,
        ];
        assert_eq!(state.encode(), bytes);
        assert_eq!(State::decode(&bytes), Some(state));
    }

    #[test]
    fn malformed_messages_are_not_interpreted_as_keyboard_state() {
        assert_eq!(State::decode(&[]), None);
        assert_eq!(State::decode(&[0; 17]), None);
        assert_eq!(State::decode(&[0; 19]), None);
        let mut bytes = [0; STATE_BYTES];
        for invalid in 2..=255 {
            bytes[16] = invalid;
            assert_eq!(State::decode(&bytes), None);
        }
    }
}
