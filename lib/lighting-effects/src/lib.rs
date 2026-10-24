//! Lighting effects: reactive magenta fade on keypress, magenta corner
//! bar, host Caps Lock indication. Pure logic; the SPI/DMA driver is in
//! the lighting task.

#![no_std]

#[cfg(test)]
extern crate std;

use node75_keyboard_core::Matrix;

pub const BAR_LEDS: usize = 5;
pub const KEY_LEDS: usize = 84;
pub const LEDS: usize = BAR_LEDS + KEY_LEDS;
pub const RESET_BYTES: usize = 200;
pub const FRAME_BYTES: usize = RESET_BYTES * 2 + LEDS * 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

const NO_LED: u8 = 0xff;
const CAPS: (usize, usize) = (3, 0);
const PEAK: u8 = 32;
const FADE_MS: u32 = 510;

// Stock's matrix-to-key-LED map; the five corner LEDs precede these on the wire.
#[rustfmt::skip]
pub static LED_MAP: [[u8; 16]; 6] = [
    [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f],
    [0x1e, 0x1d, 0x1c, 0x1b, 0x1a, 0x19, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, NO_LED, 0x10],
    [0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, NO_LED, 0x2d],
    [0x3b, 0x3a, 0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30, NO_LED, 0x2f, NO_LED, 0x2e],
    [0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, NO_LED, 0x47, 0x48, NO_LED, 0x49],
    [0x53, 0x52, 0x51, NO_LED, NO_LED, 0x50, NO_LED, NO_LED, NO_LED, 0x4f, 0x4e, 0x4d, 0x4c, 0x4b, NO_LED, 0x4a],
];

// Q65's QMK-derived CIE1931 curve, scaled to a modest raw PWM peak on this board.
const FADE: [u8; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < table.len() {
        let pwm = if i <= 20 {
            (i as u64 * 1000).div_ceil(9023)
        } else {
            (100 * i as u64 + 4080)
                .pow(3)
                .div_ceil(255_u64.pow(2) * 116_u64.pow(3))
        };
        table[i] = (pwm * PEAK as u64).div_ceil(255) as u8;
        i += 1;
    }
    table
};

const fn magenta(value: u8) -> Rgb {
    Rgb {
        red: value,
        green: 0,
        blue: value,
    }
}

#[derive(Clone, Copy)]
struct Hit {
    led: u8,
    at: u32,
}

#[derive(Default)]
pub struct Lighting {
    previous: Matrix,
    hits: [Option<Hit>; 8],
}

impl Lighting {
    pub fn update(&mut self, keys: Matrix, now: u32) {
        // Retire old timestamps so a complete timer wrap cannot revive a stale hit.
        for hit in &mut self.hits {
            if hit.is_some_and(|hit| now.wrapping_sub(hit.at) >= FADE_MS) {
                *hit = None;
            }
        }
        for (row, (&keys, leds)) in keys.iter().zip(&LED_MAP).enumerate() {
            let pressed = keys & !self.previous[row];
            for (column, &led) in leds.iter().enumerate() {
                if pressed & (1 << column) == 0 || led == NO_LED || (row, column) == CAPS {
                    continue;
                }
                self.hits.rotate_left(1);
                self.hits[7] = Some(Hit { led, at: now });
            }
        }
        self.previous = keys;
    }

    pub fn clear(&mut self, keys: Matrix) {
        self.hits = [None; 8];
        self.previous = keys;
    }

    pub fn frame(&self, now: u32, caps_lock: bool) -> [Rgb; LEDS] {
        let mut colors = [Rgb::default(); LEDS];
        colors[..BAR_LEDS].fill(magenta(4));
        for hit in self.hits.iter().flatten() {
            let faded = (now.wrapping_sub(hit.at) / 2).min(255) as usize;
            colors[BAR_LEDS + usize::from(hit.led)] = magenta(FADE[255 - faded]);
        }
        if caps_lock {
            colors[BAR_LEDS + usize::from(LED_MAP[CAPS.0][CAPS.1])] = magenta(PEAK);
        }
        colors
    }
}

pub fn encode(colors: &[Rgb; LEDS], frame: &mut [u8; FRAME_BYTES]) {
    frame.fill(0);
    // Preserve stock's framing pulses and low reset intervals.
    frame[0] = 0x80;
    frame[FRAME_BYTES - RESET_BYTES] = 0x80;
    for (color, pixel) in colors.iter().zip(frame[RESET_BYTES..].chunks_exact_mut(24)) {
        for (channel, symbols) in [color.green, color.red, color.blue]
            .into_iter()
            .zip(pixel.chunks_exact_mut(8))
        {
            for (bit, symbol) in symbols.iter_mut().enumerate() {
                *symbol = if channel & (0x80 >> bit) == 0 {
                    0xc0
                } else {
                    0xfc
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit_keys(colors: &[Rgb; LEDS]) -> std::vec::Vec<usize> {
        colors
            .iter()
            .enumerate()
            .skip(BAR_LEDS)
            .filter_map(|(i, &color)| (color != Rgb::default()).then_some(i))
            .collect()
    }

    #[test]
    fn press_fades_over_half_a_second_even_when_held_and_repress_restarts_it() {
        let mut lighting = Lighting::default();
        let a = [0, 0, 0, 2, 0, 0];
        lighting.update(a, 1000);
        let fresh = lighting.frame(1000, false);
        assert_eq!(lit_keys(&fresh), [63]);
        assert!(fresh[63].red > 0);
        assert_eq!(fresh[63].red, fresh[63].blue);
        assert_eq!(fresh[63].green, 0);
        lighting.update(a, 1300);
        let dim = lighting.frame(1300, false)[63];
        assert!(dim.red > 0 && dim.red < fresh[63].red);
        lighting.update(a, 1510);
        assert!(lit_keys(&lighting.frame(1510, false)).is_empty());
        lighting.update([0; 6], 1520);
        lighting.update(a, 1530);
        assert_eq!(lighting.frame(1530, false)[63], fresh[63]);
    }

    #[test]
    fn caps_follows_host_state_only_and_corners_stay_magenta() {
        let mut lighting = Lighting::default();
        lighting.update([0, 0, 0, 1, 0, 0], 0);
        assert!(lit_keys(&lighting.frame(0, false)).is_empty());
        let enabled = lighting.frame(0, true);
        assert_eq!(lit_keys(&enabled), [64]);
        assert_eq!(lighting.frame(10000, true), enabled);
        assert!(lit_keys(&lighting.frame(10001, false)).is_empty());
        assert!(
            enabled[..BAR_LEDS]
                .iter()
                .all(|color| color.red > 0 && color.red == color.blue && color.green == 0)
        );
    }

    #[test]
    fn fading_survives_timer_wrap_and_expired_hits_do_not_revive() {
        let mut lighting = Lighting::default();
        let start = u32::MAX - 100;
        lighting.update([1, 0, 0, 0, 0, 0], start);
        let fresh = lighting.frame(start, false)[5];
        let dim = lighting.frame(start.wrapping_add(300), false)[5];
        assert!(dim.red > 0 && dim.red < fresh.red);
        lighting.update([0; 6], start.wrapping_add(510));
        assert!(lit_keys(&lighting.frame(start, false)).is_empty());
    }

    #[test]
    fn holes_are_ignored_but_fn_is_reactive_and_sleep_forgets_presses() {
        let mut lighting = Lighting::default();
        lighting.update([0, 1 << 14, 0, 0, 0, 1 << 10], 0);
        assert_eq!(lit_keys(&lighting.frame(0, false)), [83]);
        let held = [1, 0, 0, 0, 0, 0];
        lighting.clear(held);
        lighting.update(held, 50);
        assert!(lit_keys(&lighting.frame(50, false)).is_empty());
    }

    #[test]
    fn only_the_eight_most_recent_presses_are_retained() {
        let mut lighting = Lighting::default();
        lighting.update([0x1ff, 0, 0, 0, 0, 0], 0);
        assert_eq!(
            lit_keys(&lighting.frame(0, false)),
            [6, 7, 8, 9, 10, 11, 12, 13]
        );
        assert_eq!(lit_keys(&lighting.frame(0, true)).len(), 9);
    }

    fn decode(pixel: &[u8]) -> [u8; 3] {
        let mut channels = [0; 3];
        for (channel, symbols) in channels.iter_mut().zip(pixel.chunks_exact(8)) {
            for &symbol in symbols {
                assert!(symbol == 0xc0 || symbol == 0xfc);
                *channel = (*channel << 1) | u8::from(symbol == 0xfc);
            }
        }
        channels
    }

    #[test]
    fn frame_has_stock_reset_intervals_and_exact_pixel_count() {
        let mut frame = [0xff; FRAME_BYTES];
        encode(&[Rgb::default(); LEDS], &mut frame);
        assert_eq!(frame.len(), 2536);
        assert_eq!(frame[0], 0x80);
        assert!(frame[1..200].iter().all(|&byte| byte == 0));
        assert!(frame[200..2336].iter().all(|&byte| byte == 0xc0));
        assert_eq!(frame[2336], 0x80);
        assert!(frame[2337..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn preserves_all_color_bits_in_grb_wire_order_without_crossing_pixels() {
        let mut colors = [Rgb::default(); LEDS];
        let mut frame = [0; FRAME_BYTES];
        for value in 0..=255 {
            colors[0] = Rgb {
                red: value,
                green: 0x81,
                blue: !value,
            };
            colors[88] = Rgb {
                red: 0x12,
                green: 0x34,
                blue: 0x56,
            };
            encode(&colors, &mut frame);
            assert_eq!(decode(&frame[200..224]), [0x81, value, !value]);
            assert!(frame[224..2312].iter().all(|&byte| byte == 0xc0));
            assert_eq!(decode(&frame[2312..2336]), [0x34, 0x12, 0x56]);
        }
    }
}
