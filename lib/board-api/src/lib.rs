#![no_std]

pub const MAX_I2C_BYTES: usize = 32;

include!(concat!(env!("OUT_DIR"), "/client.rs"));
