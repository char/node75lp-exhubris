/// A fixed-capacity byte buffer for assembling descriptors in const contexts.
pub struct Bytes<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Bytes<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub const fn push(&mut self, byte: u8) {
        self.bytes[self.len] = byte;
        self.len += 1;
    }

    pub const fn extend(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            self.push(data[i]);
            i += 1;
        }
    }

    /// Panics (at compile time, in a const) unless exactly `N` bytes were written.
    pub const fn finish(self) -> [u8; N] {
        assert!(self.len == N);
        self.bytes
    }
}
