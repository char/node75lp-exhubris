#![no_std]
#![no_main]

use hubris_task_slots::SLOTS;
use node75_touch_api::{Gestures, MEDIA_KEY, MediaEvent};

mod i2c;
mod sensor;

use sensor::{Sensor, Service};

const TICK: u32 = 1;
const STALE_MS: u32 = 250;
const PROBE_MS: u32 = 500;

#[unsafe(export_name = "main")]
fn main() -> ! {
    let mut sensor = unsafe { Sensor::new() };
    let mut gestures = Gestures::default();
    let mut last_sample = None;
    let mut last_probe = 0u32;
    let mut inhibited = true;
    loop {
        let now = userlib::sys_get_timer().now;
        userlib::sys_set_timer(Some(now + sensor.poll_delay()), TICK);
        userlib::sys_recv_notification(TICK);
        let sample = sensor.service();
        let now = userlib::sys_get_timer().now as u32;
        match sample {
            Service::Position(position) => {
                last_sample = position.map(|_| now);
                if position.is_none() {
                    inhibited = false;
                }
                if !inhibited {
                    let mut accepted = true;
                    gestures.update(position, now, |event| accepted &= send(event));
                    if !accepted {
                        // USB canceled the queue. Do not replay gestures after
                        // reconnect or turn the remainder of this touch into a tap.
                        gestures.cancel(|_| {});
                        inhibited = true;
                    }
                }
            }
            Service::Invalid => {
                gestures.cancel(|event| {
                    send(event);
                });
                inhibited = true;
                last_sample = None;
            }
            Service::Idle => {
                if last_sample.is_some_and(|at| now.wrapping_sub(at) >= STALE_MS) {
                    gestures.cancel(|event| {
                        send(event);
                    });
                    inhibited = true;
                    last_sample = None;
                }
                if now.wrapping_sub(last_probe) >= PROBE_MS {
                    sensor.force_window();
                    last_probe = now;
                }
            }
        }
    }
}

fn send(event: MediaEvent) -> bool {
    // The keyboard receives only from an already-blocked sender and replies
    // immediately, without waiting for the USB host.
    let mut reply = [0];
    let Ok((code, length)) = userlib::sys_send(
        SLOTS.keyboard,
        MEDIA_KEY,
        &event.encode(),
        &mut reply,
        &mut [],
    ) else {
        return false;
    };
    code == userlib::ResponseCode::SUCCESS && length == 1 && reply[0] == 1
}
