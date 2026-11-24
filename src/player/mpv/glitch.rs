//! When the stand-by card flickers, and how.
//!
//! The tuner gives the player no clock, so [`Glitch`] takes the moments from
//! the caller ([`std::time::Instant`]s from [`super::MpvPlayer::poll_event`])
//! and draws its random numbers from a seeded generator, so a test can repeat
//! a run exactly.

use std::time::{Duration, Instant};

use super::card::BAND_ROWS;

/// The shortest and longest flicker.
const SHORTEST: Duration = Duration::from_millis(80);
const LONGEST: Duration = Duration::from_millis(150);

/// The seed of the default generator: "surfcult" in ASCII.
pub(super) const SEED: u64 = 0x7375_7266_6375_6c74;

/// One flicker: one or two thin slices of the band move sideways and the
/// red and cyan copies of the text move a little further apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Flicker {
    /// The slices, which do not overlap.
    pub(super) slices: Vec<Slice>,
    /// Extra pixels between the red and cyan copies.
    pub(super) spread: u32,
}

/// Rows of the band that move sideways, in pixels of a 1920x1080 screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Slice {
    pub(super) top: u32,
    pub(super) height: u32,
    /// Pixels to the right; negative moves left.
    pub(super) shift: i32,
}

/// What [`Glitch::step`] asks the player to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// Nothing changes.
    Nothing,
    /// Draw the card with this flicker.
    Flicker(Flicker),
    /// Draw the card as it normally is.
    Settle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// The card is not up.
    Off,
    /// The card is up and the next flicker starts at this moment.
    Waiting(Instant),
    /// The card flickers until this moment.
    Flickering(Instant),
}

/// The glitch timer of one player.
#[derive(Debug, Clone)]
pub(super) struct Glitch {
    every: Duration,
    rng: u64,
    pub(super) state: State,
}

impl Glitch {
    /// A timer that flickers on average once per `every`.
    pub(super) fn new(every: Duration, seed: u64) -> Self {
        Self {
            every,
            // xorshift needs a seed other than zero.
            rng: seed.max(1),
            state: State::Off,
        }
    }

    /// The card has just been drawn at `now`: wait for the first flicker.
    pub(super) fn arm(&mut self, now: Instant) {
        self.state = State::Waiting(now + self.interval());
    }

    /// The card is gone.
    pub(super) fn disarm(&mut self) {
        self.state = State::Off;
    }

    /// What to draw at `now`.
    pub(super) fn step(&mut self, now: Instant) -> Step {
        match self.state {
            State::Waiting(at) if now >= at => {
                let length = SHORTEST + (LONGEST - SHORTEST).mul_f64(self.unit());
                self.state = State::Flickering(now + length);
                Step::Flicker(self.flicker())
            }
            State::Flickering(until) if now >= until => {
                self.state = State::Waiting(now + self.interval());
                Step::Settle
            }
            _ => Step::Nothing,
        }
    }

    /// The time to the next flicker: from half to one and a half times
    /// `every`, so the mean is `every`.
    fn interval(&mut self) -> Duration {
        self.every.mul_f64(0.5 + self.unit())
    }

    /// One slice, or two: one in the upper half of the band and one in the
    /// lower half. Each is 4 to 13 pixels high and moves 3 to 11 pixels; the
    /// colours move 2 to 6 pixels further apart.
    fn flicker(&mut self) -> Flicker {
        let (top, bottom) = BAND_ROWS;
        let middle = (top + bottom) / 2;
        let halves = match (self.unit() * 3.0) as u32 {
            0 => vec![(top, middle)],
            1 => vec![(middle, bottom)],
            _ => vec![(top, middle), (middle, bottom)],
        };
        let slices = halves
            .into_iter()
            .map(|(from, to)| {
                let height = 4 + (self.unit() * 10.0) as u32;
                let top = from + (self.unit() * f64::from(to - from - height)) as u32;
                let size = 3 + (self.unit() * 9.0) as i32;
                let shift = if self.unit() < 0.5 { -size } else { size };
                Slice { top, height, shift }
            })
            .collect();
        let spread = 2 + (self.unit() * 5.0) as u32;
        Flicker { slices, spread }
    }

    /// The next number in [0, 1), from xorshift64*.
    fn unit(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let bits = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
        bits as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flickers_come_at_random_about_every_interval_and_settle_quickly() {
        let every = Duration::from_secs(20);
        let mut glitch = Glitch::new(every, SEED);
        let start = Instant::now();
        let mut now = start;
        glitch.arm(now);
        let mut flickers = Vec::new();
        let mut waits = Vec::new();
        let mut since = None;
        // Step through ten simulated minutes, 10 ms at a time.
        while now < start + Duration::from_secs(600) {
            match glitch.step(now) {
                Step::Nothing => {}
                Step::Flicker(flicker) => {
                    since = Some(now);
                    flickers.push(flicker);
                }
                Step::Settle => {
                    let length = now - since.take().unwrap();
                    assert!(length >= SHORTEST && length <= LONGEST + Duration::from_millis(10));
                }
            }
            if let State::Waiting(at) = glitch.state
                && waits.last() != Some(&at)
            {
                waits.push(at);
            }
            now += Duration::from_millis(10);
        }
        // About 30 in ten minutes at a mean of 20 s.
        assert!((20..=40).contains(&flickers.len()), "{}", flickers.len());
        let (top, bottom) = BAND_ROWS;
        for flicker in &flickers {
            assert!((1..=2).contains(&flicker.slices.len()), "{flicker:?}");
            for slice in &flicker.slices {
                assert!((3..=11).contains(&slice.shift.abs()), "{flicker:?}");
                assert!((4..=13).contains(&slice.height), "{flicker:?}");
                assert!(
                    slice.top >= top && slice.top + slice.height <= bottom,
                    "{flicker:?}"
                );
            }
            if let [a, b] = &flicker.slices[..] {
                assert!(a.top + a.height <= b.top, "{flicker:?}");
            }
            assert!((2..=6).contains(&flicker.spread), "{flicker:?}");
        }
        assert!(flickers.iter().any(|f| f.slices.len() == 1));
        assert!(flickers.iter().any(|f| f.slices.len() == 2));
        assert!(flickers.windows(2).any(|w| w[0] != w[1]));
    }

    #[test]
    fn the_same_seed_gives_the_same_glitches() {
        let now = Instant::now();
        let run = || {
            let mut glitch = Glitch::new(Duration::from_secs(1), 7);
            glitch.arm(now);
            (1..400)
                .map(|i| glitch.step(now + Duration::from_millis(10 * i)))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn nothing_happens_while_off() {
        let mut glitch = Glitch::new(Duration::from_millis(1), SEED);
        let now = Instant::now();
        glitch.arm(now);
        glitch.disarm();
        assert_eq!(glitch.step(now + Duration::from_secs(60)), Step::Nothing);
    }
}
