#![no_std]
#![no_main]

use core::ptr::read_volatile;

use hubris_task_slots::SLOTS;
use node75_ipc::{board::Node75Board, keyboard::Node75Keyboard};
use node75_lighting_effects::Lighting;

mod lighting_hw;

const FRAME_TICK: u32 = 1;

mod board {
    pub struct Clock(pub u32);

    impl Clock {
        pub fn frequency_hz(&self) -> u32 {
            self.0
        }
    }
}

#[unsafe(export_name = "main")]
fn main() -> ! {
    let config = unsafe { read_volatile(0x4000_1008 as *const u16) };
    let divisor = u32::from(config & 0x1f);
    let frequency = match config & 0xc0 {
        0x00 if divisor != 0 => 32_000_000 / divisor,
        0x40 if divisor != 0 => 480_000_000 / divisor,
        0x80 => 32_000_000,
        _ => panic!("unsupported inherited clock"),
    };
    let keyboard = Node75Keyboard::from(SLOTS.keyboard);
    let gpio = Node75Board::from(SLOTS.board);
    let mut lights = unsafe { lighting_hw::Lights::new(&board::Clock(frequency), gpio) };
    let mut lighting = Lighting::default();
    loop {
        // Pull uphill: a lighting fault must not strand USB in Send.
        let state = keyboard.lighting_state();
        if state.active {
            lighting.update(state.keys, state.now);
        } else {
            lighting.clear(state.keys);
        }
        lights.poll(state.active, state.now, || {
            lighting.frame(state.now, state.leds & 2 != 0, state.touching)
        });
        let now = userlib::sys_get_timer().now;
        userlib::sys_set_timer(Some(now + u64::from(lighting_hw::POLL_MS)), FRAME_TICK);
        userlib::sys_recv_notification(FRAME_TICK);
    }
}
