//! Matrix state and debounce. Pure logic; no registers.

#![no_std]

pub mod keycodes;

pub const ROWS: usize = 6;
pub const COLS: usize = 16;

/// One bit per column, per row: bit `c` of `matrix[r]` is position (r, c).
pub type Matrix = [u16; ROWS];

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
