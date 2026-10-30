//! Matrix state and debounce. Pure logic; no registers.

#![no_std]

pub mod keycodes;

pub const ROWS: usize = 6;
pub const COLS: usize = 16;

/// One bit per column, per row: bit `c` of `matrix[r]` is position (r, c).
pub type Matrix = [u16; ROWS];

pub const MATRIX_SAMPLE: u16 = 1;
pub const MATRIX_SAMPLE_BYTES: usize = 4 + 2 * ROWS;

pub struct MatrixSample {
    pub keys: Matrix,
    pub now: u32,
}

impl MatrixSample {
    pub fn encode(&self) -> [u8; MATRIX_SAMPLE_BYTES] {
        let mut bytes = [0; MATRIX_SAMPLE_BYTES];
        bytes[..4].copy_from_slice(&self.now.to_le_bytes());
        for (key, bytes) in self.keys.iter().zip(bytes[4..].chunks_exact_mut(2)) {
            bytes.copy_from_slice(&key.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; MATRIX_SAMPLE_BYTES] = bytes.try_into().ok()?;
        let now = u32::from_le_bytes(bytes[..4].try_into().ok()?);
        let mut keys = [0; ROWS];
        for (key, bytes) in keys.iter_mut().zip(bytes[4..].chunks_exact(2)) {
            *key = u16::from_le_bytes([bytes[0], bytes[1]]);
        }
        Some(Self { keys, now })
    }
}

#[derive(Default)]
pub struct Debouncer {
    stable: Matrix,
    candidate: Matrix,
    changed_at: [[u32; COLS]; ROWS],
}

impl Debouncer {
    /// Accept a key once its sampled state has been unchanged for 5 ms.
    pub fn update(&mut self, raw: Matrix, now: u32) -> Matrix {
        for (row, raw) in raw.into_iter().enumerate() {
            for column in 0..COLS {
                let bit = 1 << column;
                if (raw ^ self.candidate[row]) & bit != 0 {
                    self.changed_at[row][column] = now;
                } else if now.wrapping_sub(self.changed_at[row][column]) >= 5 {
                    self.stable[row] = (self.stable[row] & !bit) | (raw & bit);
                }
            }
            self.candidate[row] = raw;
        }
        self.stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_sample_preserves_sample_time_and_every_row() {
        let sample = MatrixSample {
            now: 0x12345678,
            keys: [1, 0x8000, 0xffff, 2, 0, 0x100],
        };
        let bytes = [
            0x78, 0x56, 0x34, 0x12, 1, 0, 0, 0x80, 0xff, 0xff, 2, 0, 0, 0, 0, 1,
        ];
        assert_eq!(sample.encode(), bytes);
        let decoded = MatrixSample::decode(&bytes).unwrap();
        assert_eq!(decoded.keys, sample.keys);
        assert_eq!(decoded.now, sample.now);
        assert!(MatrixSample::decode(&bytes[..15]).is_none());
        assert!(MatrixSample::decode(&[0; 17]).is_none());
    }

    #[test]
    fn bounce_is_filtered_per_key_on_both_edges() {
        let mut debounce = Debouncer::default();
        for (time, raw, expected) in [
            (0, 1, 0),
            (1, 0, 0),
            (2, 1, 0),
            (6, 3, 0),
            (7, 3, 1),
            (11, 3, 3),
            (12, 2, 3),
            (13, 3, 3),
            (14, 2, 3),
            (19, 2, 2),
        ] {
            assert_eq!(debounce.update([raw, 0, 0, 0, 0, 0], time)[0], expected);
        }
    }

    #[test]
    fn debounce_survives_clock_wrap() {
        let mut debounce = Debouncer::default();
        assert_eq!(debounce.update([1, 0, 0, 0, 0, 0], u32::MAX - 2)[0], 0);
        for _ in 0..20 {
            assert_eq!(debounce.update([1, 0, 0, 0, 0, 0], 1)[0], 0);
        }
        assert_eq!(debounce.update([1, 0, 0, 0, 0, 0], 2)[0], 1);
    }
}
