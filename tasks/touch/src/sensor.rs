//! IQS7222C configuration recovered from stock firmware 1.0.10.6.
//! PB20/PB21 carry I2C; PA7 is the active-low ready signal.

use hubris_task_slots::SLOTS;
use node75_ipc::board::Node75Board;
const PRODUCT: u16 = 863;
const STATUS_ATI_ACTIVE: u8 = 1 << 0;
const STATUS_ATI_ERROR: u8 = 1 << 1;
const STATUS_RESET: u8 = 1 << 3;

pub enum Service {
    Idle,
    Invalid,
    Position(Option<u16>),
}

pub struct Sensor {
    board: Node75Board,
    configuration: Option<usize>,
}

impl Sensor {
    pub fn new() -> Self {
        Self {
            board: Node75Board::from(SLOTS.board),
            configuration: Some(0),
        }
    }

    pub fn poll_delay(&self) -> u64 {
        if self.configuration.is_some_and(|index| index != 0) {
            1
        } else {
            16
        }
    }

    pub fn force_window(&mut self) {
        if matches!(self.board.touch_ready(), Ok(false)) {
            let _ = self.board.i2c_write(0xff, &[]);
        }
    }

    pub fn service(&mut self) -> Service {
        match self.board.touch_ready() {
            Ok(true) => {}
            Ok(false) => {
                if self.configuration.is_some() {
                    self.configuration = Some(0);
                }
                return Service::Idle;
            }
            Err(_) => {
                self.configuration = Some(0);
                return Service::Invalid;
            }
        }
        if let Some(index) = self.configuration {
            // Bound each configuration step so other board-I/O clients can run.
            let (register, data) = CONFIG[index];
            let failed = (index == 1 && !self.identify())
                || self.board.i2c_write(register, data).is_err()
                // Retained stock state may have a 100 ms window. Allow time
                // for IPC and preemption; CONFIG restores 100 ms at the end.
                || (index == 0 && self.board.i2c_write(0xdc, &500u16.to_le_bytes()).is_err());
            if failed {
                self.configuration = Some(0);
                let _ = self.board.i2c_write(0xff, &[]);
            } else if index + 1 == CONFIG.len() {
                self.configuration = if self.board.i2c_write(0xff, &[]).is_ok() {
                    None
                } else {
                    Some(0)
                };
            } else {
                self.configuration = Some(index + 1);
            }
            return Service::Invalid;
        }
        let result = self.read_position();
        if self.board.i2c_write(0xff, &[]).is_err() {
            return Service::Invalid;
        }
        result
    }

    fn identify(&mut self) -> bool {
        let mut product = [0; 2];
        self.board.i2c_read(0x00, &mut product).is_ok() && u16::from_le_bytes(product) == PRODUCT
    }

    fn read_position(&mut self) -> Service {
        // A sensor reset restores STOP-terminated windows. Keep this window
        // open before reading identity and status in separate transactions.
        if self.board.i2c_write(0xda, &[0x0d]).is_err() || !self.identify() {
            return Service::Invalid;
        }
        let mut block = [0; 10];
        if self.board.i2c_read(0x10, &mut block).is_err() {
            return Service::Invalid;
        }
        let status = block[0];
        if status & STATUS_RESET != 0 {
            self.configuration = Some(0);
            return Service::Invalid;
        }
        if status & STATUS_ATI_ACTIVE != 0 {
            return Service::Invalid;
        }
        if status & STATUS_ATI_ERROR != 0 {
            let _ = self.board.i2c_write(0xd0, &[0xb5, 0x00]);
            return Service::Invalid;
        }
        match u16::from_le_bytes([block[8], block[9]]) {
            0xffff => Service::Position(None),
            position @ 0..=2047 => Service::Position(Some(position)),
            _ => Service::Invalid,
        }
    }
}

// man fuck it
const CONFIG: &[(u16, &[u8])] = &[
    (0x00da, &[0x0d]),
    (
        0x8000,
        &[
            0x7f, 0x0c, 0xe1, 0x7f, 0x00, 0x00, 0x7f, 0x0c, 0xe1, 0x08, 0x00, 0x00, 0x7f, 0x0c,
            0x61, 0x11, 0x00, 0x00, 0x7f, 0x0c, 0x61, 0x22, 0x00, 0x00, 0x7f, 0x0c, 0x61, 0x44,
            0x00, 0x00,
        ],
    ),
    (0x8500, &[0x8b, 0x2b, 0x10, 0x30, 0x00, 0x02]),
    (
        0x9000,
        &[
            0x0a, 0x12, 0x19, 0x00, 0x08, 0x28, 0x0a, 0x12, 0x14, 0x00, 0x08, 0x28, 0x0a, 0x12,
            0x14, 0x00, 0x08, 0x28, 0x0a, 0x12, 0x14, 0x00, 0x08, 0x28, 0x0a, 0x12, 0x14, 0x00,
            0x08, 0x28,
        ],
    ),
    (
        0x9500,
        &[
            0x0a, 0x12, 0x0a, 0x00, 0x08, 0x28, 0x0a, 0x12, 0x0e, 0x00, 0x08, 0x28, 0x0a, 0x12,
            0x0c, 0x10, 0x08, 0x28, 0x0a, 0x12, 0x0c, 0x20, 0x08, 0x28, 0x0a, 0x12, 0x0c, 0x00,
            0x08, 0x28,
        ],
    ),
    (
        0xa000,
        &[
            0xf3, 0x10, 0x3d, 0x64, 0x10, 0x31, 0x00, 0x60, 0x00, 0x00, 0x00, 0x00,
        ],
    ),
    (
        0xa100,
        &[
            0x13, 0x10, 0x45, 0x32, 0x10, 0x31, 0x00, 0x88, 0x6c, 0x04, 0x00, 0x01,
        ],
    ),
    (
        0xa200,
        &[
            0x23, 0x10, 0x45, 0x32, 0x10, 0x31, 0x00, 0x88, 0x94, 0x06, 0x02, 0x00,
        ],
    ),
    (
        0xa300,
        &[
            0x43, 0x10, 0x45, 0x32, 0x10, 0x31, 0x00, 0x90, 0xc0, 0x04, 0x00, 0x01,
        ],
    ),
    (
        0xa400,
        &[
            0x83, 0x10, 0x45, 0x32, 0x10, 0x31, 0x00, 0x90, 0x94, 0x06, 0x08, 0x00,
        ],
    ),
    (
        0xa500,
        &[
            0xf3, 0x11, 0x3d, 0x78, 0x08, 0x31, 0xf8, 0x61, 0x00, 0x00, 0x00, 0x00,
        ],
    ),
    (
        0xa600,
        &[
            0x13, 0x11, 0x45, 0x60, 0x8a, 0x31, 0xf0, 0x71, 0x3e, 0x05, 0x00, 0x01,
        ],
    ),
    (
        0xa700,
        &[
            0x23, 0x11, 0x45, 0x60, 0x4a, 0x31, 0xdc, 0x69, 0x94, 0x06, 0x40, 0x00,
        ],
    ),
    (
        0xa800,
        &[
            0x43, 0x11, 0x45, 0x64, 0x4a, 0x31, 0xf4, 0x69, 0x92, 0x05, 0x00, 0x01,
        ],
    ),
    (
        0xa900,
        &[
            0x83, 0x11, 0x45, 0x50, 0x10, 0x31, 0x00, 0x70, 0x94, 0x06, 0x00, 0x01,
        ],
    ),
    (0xaa00, &[0x12, 0x78, 0x34, 0x01]),
    (
        0xb000,
        &[
            0x54, 0x2a, 0x4c, 0x01, 0xc8, 0x00, 0x00, 0x08, 0xc0, 0x03, 0x96, 0x06,
        ],
    ),
    (0xb006, &[0x34, 0x05, 0x5e, 0x05, 0x88, 0x05, 0xb2, 0x05]),
    (
        0xb100,
        &[
            0x50, 0x3c, 0x46, 0x01, 0xc8, 0x00, 0xd0, 0x07, 0x00, 0x00, 0x96, 0x06,
        ],
    ),
    (0xb106, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (
        0xc000,
        &[
            0x00, 0x00, 0x01, 0x00, 0x96, 0x06, 0x00, 0x00, 0x20, 0x00, 0x96, 0x06, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
    ),
    (
        0x00d0,
        &[
            0xb5, 0x00, 0x02, 0x00, 0x00, 0x00, 0x88, 0x13, 0x10, 0x00, 0x88, 0x13, 0x3c, 0x00,
            0x10, 0x27, 0x60, 0x00, 0xfe, 0xcf,
        ],
    ),
    (0x00db, &[0x00]),
    (0x00dc, &[0x64, 0x00]),
];
