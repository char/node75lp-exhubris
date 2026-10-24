use node75_keyboard_core::{COLS, Matrix, ROWS, keycodes::*};
use node75_usb::Report;

const NO: u8 = 0;

#[rustfmt::skip]
const LAYOUT: [[u8; COLS]; ROWS] = [
    [ESC,  F1,   F2,   F3,  F4,  F5,  F6,  F7,  F8,   F9,   F10,  F11,  F12,  PSCR, INS, DEL ],
    [GRV,  N1,   N2,   N3,  N4,  N5,  N6,  N7,  N8,   N9,   N0,   MINS, EQL,  BSPC, NO,  HOME],
    [TAB,  Q,    W,    E,   R,   T,   Y,   U,   I,    O,    P,    LBRC, RBRC, BSLS, NO,  END ],
    [CAPS, A,    S,    D,   F,   G,   H,   J,   K,    L,    SCLN, QUOT, NO,   ENT,  NO,  PGUP],
    [LSFT, Z,    X,    C,   V,   B,   N,   M,   COMM, DOT,  SLSH, NO,   RSFT, UP,   NO,  PGDN],
    [LCTL, LGUI, LALT, NO,  NO,  SPC, NO,  NO,  NO,   RALT, NO,   RCTL, LEFT, DOWN, NO,  RGHT],
];

pub fn report(keys: Matrix) -> Report {
    let pressed = || {
        (0..ROWS)
            .flat_map(move |row| (0..COLS).map(move |column| (row, column)))
            .filter(move |&(row, column)| keys[row] & 1 << column != 0)
            .map(|(row, column)| LAYOUT[row][column])
            .filter(|&usage| usage != NO)
    };
    let modifiers = pressed()
        .filter(|usage| (LCTL..=RGUI).contains(usage))
        .fold(0, |modifiers, usage| modifiers | 1 << (usage - LCTL));
    node75_keyboard_core::report(
        modifiers,
        pressed().filter(|usage| !(LCTL..=RGUI).contains(usage)),
    )
}
