// fully vibed gesture detection

#![no_std]

#[cfg(test)]
extern crate std;

/// IPC operation: touch task → keyboard task, payload [`MediaEvent`].
pub const MEDIA_KEY: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    NextTrack,
    PrevTrack,
    PlayPause,
    VolumeUp,
    VolumeDown,
}

impl Media {
    /// HID consumer-page usage ID.
    pub fn usage(self) -> u16 {
        match self {
            Media::NextTrack => 0x00b5,
            Media::PrevTrack => 0x00b6,
            Media::PlayPause => 0x00cd,
            Media::VolumeUp => 0x00e9,
            Media::VolumeDown => 0x00ea,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            0 => Media::NextTrack,
            1 => Media::PrevTrack,
            2 => Media::PlayPause,
            3 => Media::VolumeUp,
            4 => Media::VolumeDown,
            _ => return None,
        })
    }
}

/// One key transition, as sent over IPC: `[tag, pressed]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaEvent {
    pub key: Media,
    pub pressed: bool,
}

impl MediaEvent {
    pub fn encode(self) -> [u8; 2] {
        [self.key as u8, u8::from(self.pressed)]
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let [tag, pressed] = <[u8; 2]>::try_from(bytes).ok()?;
        Some(Self {
            key: Media::from_tag(tag)?,
            pressed: match pressed {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

/// Travel from touch-down position that counts as a swipe.
pub const SWIPE_MIN: u16 = 350;
/// Hold time before volume engages.
pub const HOLD_MS: u32 = 400;
/// Max gap between the two tap releases of a double tap.
pub const DOUBLE_TAP_MS: u32 = 350;
/// Positions below this are the left half (volume down on hold).
pub const MIDPOINT: u16 = 1024;

#[derive(Default)]
pub struct Gestures {
    touch: Option<Touch>,
    last_tap_at: Option<u32>,
}

struct Touch {
    start: u16,
    since: u32,
    phase: Phase,
}

#[derive(Clone, Copy)]
enum Phase {
    Tracking,
    Swiped,
    Holding(Media),
}

impl Gestures {
    /// A missing or reset sensor is not a tap. Release any hold and forget
    /// the partial gesture before accepting fresh samples.
    pub fn cancel(&mut self, mut emit: impl FnMut(MediaEvent)) {
        if let Some(Touch {
            phase: Phase::Holding(key),
            ..
        }) = self.touch.take()
        {
            emit(MediaEvent {
                key,
                pressed: false,
            });
        }
        self.last_tap_at = None;
    }

    pub fn update(&mut self, position: Option<u16>, now: u32, mut emit: impl FnMut(MediaEvent)) {
        match (self.touch.as_mut(), position) {
            (None, Some(start)) => {
                self.touch = Some(Touch {
                    start,
                    since: now,
                    phase: Phase::Tracking,
                });
            }
            (Some(touch), Some(pos)) => {
                let travel = pos as i32 - touch.start as i32;
                if matches!(touch.phase, Phase::Tracking) {
                    if travel >= i32::from(SWIPE_MIN) {
                        Self::oneshot(&mut emit, Media::NextTrack);
                        touch.phase = Phase::Swiped;
                        self.last_tap_at = None;
                    } else if travel <= -i32::from(SWIPE_MIN) {
                        Self::oneshot(&mut emit, Media::PrevTrack);
                        touch.phase = Phase::Swiped;
                        self.last_tap_at = None;
                    } else if now.wrapping_sub(touch.since) >= HOLD_MS {
                        let key = if touch.start < MIDPOINT {
                            Media::VolumeDown
                        } else {
                            Media::VolumeUp
                        };
                        emit(MediaEvent { key, pressed: true });
                        touch.phase = Phase::Holding(key);
                        self.last_tap_at = None;
                    }
                }
            }
            (Some(_), None) => {
                let touch = self.touch.take().expect("matched Some");
                if let Phase::Holding(key) = touch.phase {
                    emit(MediaEvent {
                        key,
                        pressed: false,
                    });
                } else if matches!(touch.phase, Phase::Tracking)
                    && now.wrapping_sub(touch.since) < HOLD_MS
                {
                    if self
                        .last_tap_at
                        .is_some_and(|at| now.wrapping_sub(at) <= DOUBLE_TAP_MS)
                    {
                        Self::oneshot(&mut emit, Media::PlayPause);
                        self.last_tap_at = None;
                    } else {
                        self.last_tap_at = Some(now);
                    }
                } else {
                    self.last_tap_at = None;
                }
            }
            (None, None) => {}
        }
    }

    fn oneshot(emit: &mut impl FnMut(MediaEvent), key: Media) {
        emit(MediaEvent { key, pressed: true });
        emit(MediaEvent {
            key,
            pressed: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(gestures: &mut Gestures, samples: &[(Option<u16>, u32)]) -> std::vec::Vec<MediaEvent> {
        let mut out = std::vec::Vec::new();
        for &(pos, now) in samples {
            gestures.update(pos, now, |e| out.push(e));
        }
        out
    }

    fn ev(key: Media, pressed: bool) -> MediaEvent {
        MediaEvent { key, pressed }
    }

    #[test]
    fn swipe_right_is_next_track_once() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[
                (Some(200), 0),
                (Some(400), 16),
                (Some(700), 32),
                (Some(1500), 48),
                (None, 64),
            ],
        );
        assert_eq!(
            out,
            [ev(Media::NextTrack, true), ev(Media::NextTrack, false)]
        );
    }

    #[test]
    fn swipe_left_is_prev_track() {
        let mut g = Gestures::default();
        let out = run(&mut g, &[(Some(1800), 0), (Some(1200), 16), (None, 32)]);
        assert_eq!(
            out,
            [ev(Media::PrevTrack, true), ev(Media::PrevTrack, false)]
        );
    }

    #[test]
    fn small_movements_do_not_swipe() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[
                (Some(500), 0),
                (Some(700), 50),
                (Some(400), 100),
                (None, 150),
            ],
        );
        assert!(out.is_empty());
    }

    #[test]
    fn double_tap_plays_pauses_single_tap_does_nothing() {
        let mut g = Gestures::default();
        assert!(run(&mut g, &[(Some(900), 0), (None, 80)]).is_empty());
        let out = run(&mut g, &[(Some(900), 200), (None, 280)]);
        assert_eq!(
            out,
            [ev(Media::PlayPause, true), ev(Media::PlayPause, false)]
        );
        // The pair is consumed: a third tap alone does nothing.
        assert!(run(&mut g, &[(Some(900), 400), (None, 480)]).is_empty());
    }

    #[test]
    fn taps_too_far_apart_are_not_a_double_tap() {
        let mut g = Gestures::default();
        run(&mut g, &[(Some(900), 0), (None, 80)]);
        assert!(run(&mut g, &[(Some(900), 1000), (None, 1080)]).is_empty());
    }

    #[test]
    fn hold_right_side_holds_volume_up_until_release() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[
                (Some(1700), 0),
                (Some(1710), 200),
                (Some(1690), 400),
                (Some(1700), 900),
                (None, 1000),
            ],
        );
        assert_eq!(out, [ev(Media::VolumeUp, true), ev(Media::VolumeUp, false)]);
    }

    #[test]
    fn hold_left_side_is_volume_down_and_slide_after_hold_does_not_swipe() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[
                (Some(300), 0),
                (Some(300), 500),
                (Some(1800), 700),
                (None, 800),
            ],
        );
        assert_eq!(
            out,
            [ev(Media::VolumeDown, true), ev(Media::VolumeDown, false)]
        );
    }

    #[test]
    fn swipe_between_taps_breaks_the_pair() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[
                (Some(200), 0),
                (None, 20),
                (Some(200), 40),
                (Some(800), 60),
                (None, 80),
                (Some(200), 100),
                (None, 120),
            ],
        );
        assert_eq!(
            out,
            [ev(Media::NextTrack, true), ev(Media::NextTrack, false)]
        );
    }

    #[test]
    fn a_long_touch_without_intermediate_samples_is_not_a_tap() {
        let mut g = Gestures::default();
        assert!(
            run(
                &mut g,
                &[(Some(900), 0), (None, 500), (Some(900), 600), (None, 650),]
            )
            .is_empty()
        );
    }

    #[test]
    fn cancel_releases_holds_without_synthesizing_taps() {
        let mut g = Gestures::default();
        run(&mut g, &[(Some(1800), 0), (Some(1800), 400)]);
        let mut out = std::vec::Vec::new();
        g.cancel(|e| out.push(e));
        g.cancel(|e| out.push(e));
        assert_eq!(out, [ev(Media::VolumeUp, false)]);
        assert!(run(&mut g, &[(Some(900), 500), (None, 550)]).is_empty());
        g.cancel(|_| panic!("a tap has no held key"));
        assert!(run(&mut g, &[(Some(900), 600), (None, 650)]).is_empty());
    }

    #[test]
    fn gesture_timing_wraps() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            &[(Some(100), u32::MAX - 200), (Some(100), 199), (None, 200)],
        );
        assert_eq!(
            out,
            [ev(Media::VolumeDown, true), ev(Media::VolumeDown, false)]
        );
    }

    #[test]
    fn wire_codec_roundtrips_and_rejects_garbage() {
        for key in [
            Media::NextTrack,
            Media::PrevTrack,
            Media::PlayPause,
            Media::VolumeUp,
            Media::VolumeDown,
        ] {
            let event = ev(key, true);
            assert_eq!(MediaEvent::decode(&event.encode()), Some(event));
        }
        assert_eq!(MediaEvent::decode(&[9, 1]), None);
        assert_eq!(MediaEvent::decode(&[0, 2]), None);
        assert_eq!(MediaEvent::decode(&[]), None);
    }
}
