//! Generated clients for this application's task interfaces.

#![no_std]

pub mod board {
    pub const MAX_I2C_BYTES: usize = 32;

    include!(concat!(env!("OUT_DIR"), "/board.rs"));
}

pub mod keyboard {
    include!(concat!(env!("OUT_DIR"), "/keyboard.rs"));
}
