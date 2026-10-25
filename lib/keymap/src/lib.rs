#![no_std]

#[cfg(test)]
extern crate std;

use node75_keyboard_core::{
    COLS, Matrix, ROWS,
    keycodes::{self, *},
};

const KEYS: usize = ROWS * COLS;

pub const TAPPING_TERM_MS: u32 = 200; // tap vs hold threshold for TapHold

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// No key at this position on this layer; blocks fallthrough.
    None,
    /// Fall through to the next lower active layer.
    Transparent,
    /// A HID keyboard usage ID (see [`keycodes`]).
    Key(u8),
    /// Activate layer `n` while held.
    Momentary(u8),
    /// Flip layer `n`'s toggled state on press.
    Toggle(u8),
    /// Send usage `k` when tapped, activate layer `n` while held.
    TapHold(u8, u8),
}

use Action::{Key as K, Momentary, None as No, TapHold, Toggle, Transparent as Tr};

impl Action {
    /// [`PartialEq`] on `Action` is not `const`, and layer lookup needs it.
    const fn same(self, other: Self) -> bool {
        match (self, other) {
            (No, No) | (Tr, Tr) => true,
            (K(a), K(b)) | (Momentary(a), Momentary(b)) | (Toggle(a), Toggle(b)) => a == b,
            (TapHold(key, layer), TapHold(other_key, other_layer)) => {
                key == other_key && layer == other_layer
            }
            _ => false,
        }
    }
}

type Grid = [[Action; COLS]; ROWS];

const BLANK: Grid = [[Tr; COLS]; ROWS];

const fn layer<const N: usize>(mut grid: Grid, base: &Grid, swaps: [(Action, Action); N]) -> Grid {
    // epic triple nested loop

    let mut i = 0;
    while i < N {
        let (from, to) = swaps[i];
        let mut found = false;
        let mut row = 0;
        'search: while row < ROWS {
            let mut col = 0;
            while col < COLS {
                if base[row][col].same(from) {
                    grid[row][col] = to;
                    found = true;
                    break 'search;
                }
                col += 1;
            }
            row += 1;
        }
        assert!(found, "layer: no base binding matches a swap source");
        i += 1;
    }
    grid
}

#[derive(Clone, Copy)]
struct Layer {
    grid: &'static Grid,
    /// Releasing a momentary activation of this layer clears the toggle.
    ends_with: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Modifier bitmask: bit `i` is usage `0xe0 + i`, as in a HID report.
    pub modifiers: u8,
    usages: [u32; 4],
}

impl Resolved {
    pub fn insert(&mut self, usage: u8) {
        match usage {
            0xe0..=0xe7 => self.modifiers |= 1 << (usage - 0xe0),
            4..=0x73 => self.usages[usize::from(usage / 32)] |= 1 << (usage % 32),
            _ => {}
        }
    }

    pub fn contains(&self, usage: u8) -> bool {
        (4..=0x73).contains(&usage)
            && self.usages[usize::from(usage / 32)] & (1 << (usage % 32)) != 0
    }

    /// Non-modifier usages pressed, in ascending usage order.
    pub fn usages(&self) -> impl Iterator<Item = u8> + '_ {
        (4..=0x73u8).filter(|&usage| self.contains(usage))
    }
}

#[derive(Clone, Copy, Default)]
struct Event {
    position: usize,
    pressed: bool,
}

#[derive(Clone, Copy)]
struct Pending {
    position: usize,
    since: u32,
    key: u8,
    layer: u8,
}

/// while we're still deciding what to do with a tap-hold key, we buffer events here
#[derive(Default)]
struct Buffered {
    events: [Event; 16],
    len: usize,
}

impl Buffered {
    fn push(&mut self, event: Event) -> bool {
        let Some(slot) = self.events.get_mut(self.len) else {
            return false;
        };
        *slot = event;
        self.len += 1;
        true
    }

    fn drain(&mut self) -> impl Iterator<Item = Event> + use<> {
        let events = self.events;
        events.into_iter().take(core::mem::take(&mut self.len))
    }
}

pub struct Keymap {
    layers: &'static [Layer],
    previous: Matrix,
    held: [Action; KEYS],
    toggled: u32,
    pending: Option<Pending>,
    buffered: Buffered,
}

impl Keymap {
    pub fn new() -> Self {
        Self::with_layers(&LAYERS)
    }

    fn with_layers(layers: &'static [Layer]) -> Self {
        Self {
            layers,
            previous: [0; ROWS],
            held: [Action::None; KEYS],
            toggled: 0,
            pending: None,
            buffered: Buffered::default(),
        }
    }

    /// Feed one debounced scan. `emit` is called with the resolved keys
    /// whenever they change, in order (a tap emits its key down and up).
    pub fn update(&mut self, matrix: Matrix, now: u32, mut emit: impl FnMut(Resolved)) {
        let mut last = self.resolved();
        let mut emit = |resolved: Resolved| {
            if resolved != last {
                emit(resolved);
                last = resolved;
            }
        };
        if let Some(pending) = self.pending
            && now.wrapping_sub(pending.since) >= TAPPING_TERM_MS
        {
            self.resolve_hold(&mut emit);
        }
        // we store "early" presses to prioritize modifiers and layers
        // that occur in the same scan so e.g. Shift+A doesn't come out
        // to lowercase a, Shift
        let mut early = [0; ROWS];
        for position in 0..KEYS {
            let (row, bit) = (position / COLS, 1 << (position % COLS));
            if matrix[row] & !self.previous[row] & bit != 0
                && matches!(
                    self.binding(position),
                    Momentary(_) | Toggle(_) | TapHold(..) | K(LCTL..=RGUI)
                )
            {
                early[row] |= bit;
            }
        }
        let late = core::array::from_fn(|row| matrix[row] & !self.previous[row] & !early[row]);
        let released = core::array::from_fn(|row| self.previous[row] & !matrix[row]);
        for group in [early, late, released] {
            for position in 0..KEYS {
                let (row, bit) = (position / COLS, 1 << (position % COLS));
                if group[row] & bit != 0 {
                    let pressed = matrix[row] & bit != 0;
                    self.event(Event { position, pressed }, now, &mut emit);
                }
            }
        }
        self.previous = matrix;
    }

    fn resolved(&self) -> Resolved {
        let mut resolved = Resolved::default();
        for action in self.held {
            if let Action::Key(usage) = action {
                resolved.insert(usage);
            }
        }
        resolved
    }

    fn layer_active(&self, layer: u8) -> bool {
        let bit = 1u32 << layer;
        self.toggled & bit != 0 || self.held.contains(&Action::Momentary(layer))
    }

    fn binding(&self, position: usize) -> Action {
        let (row, col) = (position / COLS, position % COLS);
        for (index, layer) in self.layers.iter().enumerate().rev() {
            if index != 0 && !self.layer_active(index as u8) {
                continue;
            }
            match layer.grid[row][col] {
                Action::Transparent => continue,
                action => return action,
            }
        }
        Action::None
    }

    fn event(&mut self, event: Event, now: u32, emit: &mut impl FnMut(Resolved)) {
        if let Some(pending) = self.pending {
            if event.position == pending.position && !event.pressed {
                self.resolve_tap(emit);
                return;
            }
            if !event.pressed && matches!(self.held[event.position], Action::Key(_)) {
                self.apply(event, emit);
                return;
            }
            if self.buffered.push(event) {
                return;
            }
            self.resolve_hold(emit);
        }
        if event.pressed
            && let Action::TapHold(key, layer) = self.binding(event.position)
        {
            self.held[event.position] = Action::TapHold(key, layer);
            self.pending = Some(Pending {
                position: event.position,
                since: now,
                key,
                layer,
            });
            return;
        }
        self.apply(event, emit);
    }

    fn apply(&mut self, event: Event, emit: &mut impl FnMut(Resolved)) {
        let binding = if event.pressed {
            self.binding(event.position)
        } else {
            self.held[event.position]
        };
        self.held[event.position] = if event.pressed { binding } else { Action::None };
        match binding {
            Action::Toggle(layer) if event.pressed => self.toggled ^= 1 << layer,
            Action::Momentary(released) if !event.pressed => {
                for (index, layer) in self.layers.iter().enumerate() {
                    if layer.ends_with == Some(released) {
                        self.toggled &= !(1 << index);
                    }
                }
            }
            _ => {}
        }
        emit(self.resolved());
    }

    fn resolve_tap(&mut self, emit: &mut impl FnMut(Resolved)) {
        let pending = self.pending.take().expect("tap without pending");
        self.held[pending.position] = Action::Key(pending.key);
        emit(self.resolved());
        self.held[pending.position] = Action::None;
        emit(self.resolved());
        for event in self.buffered.drain() {
            self.apply(event, emit);
        }
    }

    fn resolve_hold(&mut self, emit: &mut impl FnMut(Resolved)) {
        let pending = self.pending.take().expect("hold without pending");
        self.held[pending.position] = Action::Momentary(pending.layer);
        for event in self.buffered.drain() {
            self.apply(event, emit);
        }
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

const NAV_LAYER: u8 = 1;
const EXT_LAYER: u8 = 2;
const FKEY_LAYER: u8 = 3;

const LAYERS: [Layer; 4] = [
    Layer {
        grid: &BASE,
        ends_with: None,
    },
    Layer {
        grid: &NAV,
        ends_with: None,
    },
    Layer {
        grid: &EXT,
        ends_with: Some(NAV_LAYER),
    },
    Layer {
        grid: &FKEY,
        ends_with: None,
    },
];

/// Tap for Caps Lock, hold for the function layer.
const FN_KEY: Action = TapHold(CAPS, FKEY_LAYER);

#[rustfmt::skip]
const BASE: Grid = [
    [K(ESC),  K(F1),  K(F2),  K(F3),  K(F4),   K(F5),   K(F6),  K(F7),  K(F8),   K(F9),   K(F10),     K(F11),     K(F12),     K(PSCR), K(INS),  K(DEL) ],
    [K(GRV),  K(N1),  K(N2),  K(N3),  K(N4),   K(N5),   K(N6),  K(N7),  K(N8),   K(N9),   K(N0),      K(MINS),    K(EQL),     K(BSPC), No,      K(HOME)],
    [K(TAB),  K(Q),   K(W),   K(E),   K(R),    K(T),    K(Y),   K(U),   K(I),    K(O),    K(P),       K(LBRC),    K(RBRC),    K(BSLS), No,      K(END) ],
    [FN_KEY,  K(A),   K(S),   K(D),   K(F),    K(G),    K(H),   K(J),   K(keycodes::K), K(L), K(SCLN), K(QUOT), No, K(ENT), No, K(PGUP)],
    [K(LSFT), K(Z),   K(X),   K(C),   K(V),    K(B),    K(N),   K(M),   K(COMM), K(DOT),  K(SLSH),    No,         K(RSFT),    K(UP),   No,      K(PGDN)],
    [K(LCTL), K(LGUI), K(LALT), No,   No,     K(SPC),   No,     No,     No,      K(RALT), Momentary(NAV_LAYER), K(RCTL), K(LEFT), K(DOWN), No, K(RGHT)],
];

const FKEY: Grid = layer(
    BLANK,
    &BASE,
    [
        (K(F1), K(F13)),
        (K(F2), K(F14)),
        (K(F3), K(F15)),
        (K(F4), K(F16)),
        (K(F5), K(F17)),
        (K(F6), K(F18)),
        (K(F7), K(F19)),
        (K(F8), K(F20)),
        (K(F9), K(F21)),
        (K(F10), K(F22)),
        (K(F11), K(F23)),
        (K(F12), K(F24)),
    ],
);

const EXT: Grid = layer(
    FKEY,
    &BASE,
    [
        (K(Q), K(INS)),
        (K(W), K(PGUP)),
        (K(E), K(SCRL)),
        (K(A), K(HOME)),
        (K(S), K(PGDN)),
        (K(D), K(END)),
    ],
);

const NAV: Grid = layer(
    BLANK,
    &BASE,
    [
        (K(ESC), K(GRV)),
        (K(BSPC), K(DEL)),
        (K(TAB), K(PSCR)),
        (K(W), K(UP)),
        (FN_KEY, Toggle(EXT_LAYER)),
        (K(A), K(LEFT)),
        (K(S), K(DOWN)),
        (K(D), K(RGHT)),
    ],
);

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    const FN: (usize, usize) = (5, 10);
    const CAPS_POS: (usize, usize) = (3, 0);
    const W_POS: (usize, usize) = (2, 2);
    const A_POS: (usize, usize) = (3, 1);
    const F1_POS: (usize, usize) = (0, 1);
    const Q_POS: (usize, usize) = (2, 1);

    fn scan(keymap: &mut Keymap, pressed: &[(usize, usize)], now: u32) -> Vec<Resolved> {
        let mut matrix = [0; ROWS];
        for &(row, col) in pressed {
            matrix[row] |= 1 << col;
        }
        let mut emitted = Vec::new();
        keymap.update(matrix, now, |resolved| emitted.push(resolved));
        emitted
    }

    fn last(emitted: &[Resolved]) -> Resolved {
        emitted.last().copied().unwrap_or_default()
    }

    #[test]
    fn base_keys_and_modifiers() {
        let mut keymap = Keymap::new();
        let out = scan(&mut keymap, &[(3, 1), (5, 0)], 0);
        assert!(last(&out).contains(A));
        assert_eq!(last(&out).modifiers, 0b1);
        let out = scan(&mut keymap, &[], 1);
        assert_eq!(last(&out), Resolved::default());
    }

    #[test]
    fn shift_and_key_in_the_same_scan_never_report_unshifted() {
        let mut keymap = Keymap::new();
        let out = scan(&mut keymap, &[(4, 0), (3, 1)], 0); // LShift + A together
        assert!(
            out.iter()
                .all(|r| !r.contains(A) || r.modifiers & 0b10 != 0)
        );
        assert!(last(&out).contains(A));
        assert_ne!(last(&out).modifiers & 0b10, 0);
    }

    #[test]
    fn fn_turns_wasd_into_arrows() {
        let mut keymap = Keymap::new();
        scan(&mut keymap, &[FN], 0);
        let out = scan(&mut keymap, &[FN, W_POS, A_POS], 1);
        assert!(last(&out).contains(UP));
        assert!(last(&out).contains(LEFT));
        // Still held, so still Up: held bindings are remembered.
        let out = scan(&mut keymap, &[W_POS], 2);
        assert!(last(&out).contains(UP));
        scan(&mut keymap, &[], 3);
        let out = scan(&mut keymap, &[W_POS], 4);
        assert!(last(&out).contains(W));
    }

    #[test]
    fn caps_tap_sends_caps_lock_hold_does_not() {
        let mut keymap = Keymap::new();
        scan(&mut keymap, &[CAPS_POS], 0);
        let out = scan(&mut keymap, &[], 50);
        assert!(out.iter().any(|r| r.contains(CAPS)));
        assert_eq!(last(&out), Resolved::default());

        let mut keymap = Keymap::new();
        scan(&mut keymap, &[CAPS_POS], 0);
        let out = scan(&mut keymap, &[CAPS_POS], 250);
        assert!(!out.iter().any(|r| r.contains(CAPS)));
    }

    #[test]
    fn caps_hold_turns_f_row_into_f13_to_f24() {
        let mut keymap = Keymap::new();
        scan(&mut keymap, &[CAPS_POS], 0);
        scan(&mut keymap, &[CAPS_POS], 250);
        let out = scan(&mut keymap, &[CAPS_POS, F1_POS], 260);
        assert!(last(&out).contains(F13));
        let out = scan(&mut keymap, &[], 300);
        assert_eq!(last(&out), Resolved::default());
    }

    #[test]
    fn ext_toggles_from_nav_and_ends_with_fn() {
        let mut keymap = Keymap::new();
        scan(&mut keymap, &[FN], 0);
        scan(&mut keymap, &[FN, CAPS_POS], 10);
        scan(&mut keymap, &[FN], 20);
        let out = scan(&mut keymap, &[FN, Q_POS], 30);
        assert!(last(&out).contains(INS));
        let out = scan(&mut keymap, &[FN, F1_POS], 40);
        assert!(last(&out).contains(F13));
        scan(&mut keymap, &[], 50);
        let out = scan(&mut keymap, &[Q_POS], 60);
        assert!(last(&out).contains(Q));
    }

    #[test]
    fn a_layer_pressed_in_the_same_scan_cannot_swallow_a_press() {
        const BASE: Grid = {
            let mut grid = [[No; COLS]; ROWS];
            grid[0][0] = K(A);
            grid[0][1] = Momentary(1);
            grid
        };
        const MODS: Grid = layer(BLANK, &BASE, [(K(A), K(LSFT))]);
        const LAYERS: [Layer; 2] = [
            Layer {
                grid: &BASE,
                ends_with: None,
            },
            Layer {
                grid: &MODS,
                ends_with: None,
            },
        ];
        let mut keymap = Keymap::with_layers(&LAYERS);
        let out = scan(&mut keymap, &[(0, 0), (0, 1)], 0);
        assert_eq!(last(&out).modifiers, 0b10);
        let out = scan(&mut keymap, &[], 1);
        assert_eq!(last(&out), Resolved::default());
    }

    #[test]
    fn a_toggle_press_is_not_also_the_key_it_reveals() {
        const BASE: Grid = {
            let mut grid = [[No; COLS]; ROWS];
            grid[0][0] = Momentary(1);
            grid[0][1] = K(B);
            grid
        };
        const NAV: Grid = layer(BLANK, &BASE, [(K(B), Toggle(2))]);
        const EXT: Grid = layer(BLANK, &BASE, [(K(B), K(CAPS))]);
        const LAYERS: [Layer; 3] = [
            Layer {
                grid: &BASE,
                ends_with: None,
            },
            Layer {
                grid: &NAV,
                ends_with: None,
            },
            Layer {
                grid: &EXT,
                ends_with: Some(1),
            },
        ];
        let mut keymap = Keymap::with_layers(&LAYERS);
        scan(&mut keymap, &[(0, 0)], 0);
        let out = scan(&mut keymap, &[(0, 0), (0, 1)], 1);
        assert!(!out.iter().any(|r| r.contains(CAPS)));
        scan(&mut keymap, &[(0, 0)], 2);
        let out = scan(&mut keymap, &[(0, 0), (0, 1)], 3);
        assert!(last(&out).contains(CAPS));
    }

    #[test]
    fn keys_pressed_during_caps_decision_resolve_after_it() {
        let mut keymap = Keymap::new();
        scan(&mut keymap, &[CAPS_POS], 0);
        scan(&mut keymap, &[CAPS_POS, A_POS], 10);
        let out = scan(&mut keymap, &[CAPS_POS, A_POS], 250);
        assert!(last(&out).contains(A));
        assert!(!out.iter().any(|r| r.contains(CAPS)));
    }
}
