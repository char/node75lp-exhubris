#![no_std]
#![no_main]

use hubris_task_slots::SLOTS;
use node75_ipc::keyboard::Node75Keyboard;
use node75_touch_api::{Gestures, MediaEvent};

mod sensor;

use sensor::{Sensor, Service};

const TICK: u32 = 1;
const STALE_MS: u32 = 250;
const PROBE_MS: u32 = 500;

#[unsafe(export_name = "main")]
fn main() -> ! {
    let keyboard = Node75Keyboard::from(SLOTS.keyboard);
    let mut sensor = Sensor::new();
    let mut gestures = Gestures::default();
    let mut last_sample = None;
    let mut last_probe = 0u32;
    let mut inhibited = true;
    let mut touching = false;
    loop {
        let now = userlib::sys_get_timer().now;
        userlib::sys_set_timer(Some(now + sensor.poll_delay()), TICK);
        userlib::sys_recv_notification(TICK);
        let sample = sensor.service();
        let now = userlib::sys_get_timer().now as u32;
        match sample {
            Service::Position(position) => {
                report_contact(&keyboard, &mut touching, position.is_some());
                last_sample = position.map(|_| now);
                if position.is_none() {
                    inhibited = false;
                }
                if !inhibited {
                    let mut accepted = true;
                    gestures.update(position, now, |event| accepted &= send(&keyboard, event));
                    if !accepted {
                        // USB canceled the queue. Do not replay gestures after
                        // reconnect or turn the remainder of this touch into a tap.
                        gestures.cancel(|_| {});
                        inhibited = true;
                    }
                }
            }
            Service::Invalid => {
                report_contact(&keyboard, &mut touching, false);
                gestures.cancel(|event| {
                    send(&keyboard, event);
                });
                inhibited = true;
                last_sample = None;
            }
            Service::Idle => {
                if last_sample.is_some_and(|at| now.wrapping_sub(at) >= STALE_MS) {
                    report_contact(&keyboard, &mut touching, false);
                    gestures.cancel(|event| {
                        send(&keyboard, event);
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

/// Tell the keyboard when a finger lands on or leaves the bar, for lighting.
fn report_contact(keyboard: &Node75Keyboard, touching: &mut bool, contact: bool) {
    if *touching != contact {
        *touching = contact;
        keyboard.set_contact(contact);
    }
}

/// False if USB discarded the gesture queue.
fn send(keyboard: &Node75Keyboard, event: MediaEvent) -> bool {
    keyboard.media_key(if event.pressed { event.key.usage() } else { 0 })
}
