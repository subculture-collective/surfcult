//! When the stand-by card's text glitches, and how.
//!
//! The tuner gives the player no clock, so [`Glitch`] takes the moments from
//! the caller ([`std::time::Instant`]s from [`super::MpvPlayer::poll_event`])
//! and draws its random numbers from a seeded generator, so a test can repeat
//! a run exactly.

use std::time::{Duration, Instant};

/// The shortest and longest glitch.
const SHORTEST: Duration = Duration::from_millis(100);
const LONGEST: Duration = Duration::from_millis(200);

/// The seed of the default generator: "surfcult" in ASCII.
pub(super) const SEED: u64 = 0x7375_7266_6375_6c74;

/// How many horizontal strips the torn band is cut into.
pub(super) const STRIPS: usize = 4;

/// One glitch: how far each strip of the band moves sideways and how far
/// apart the red and cyan copies of the text jump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tear {
    /// Sideways offset of each strip, top to bottom, in pixels of a
    /// 1920x1080 screen.
    pub(super) strips: [i32; STRIPS],
    /// How far the red and cyan copies sit from the white one.
    pub(super) chroma: u32,
}

/// What [`Glitch::step`] asks the player to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// Nothing changes.
    Nothing,
    /// Draw the band torn.
    Tear(Tear),
    /// Draw the band as it normally is.
    Settle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// The card is not up.
    Off,
    /// The card is up and the next glitch starts at this moment.
    Waiting(Instant),
    /// The band is torn until this moment.
    Tearing(Instant),
}

/// The glitch timer of one player.
#[derive(Debug, Clone)]
pub(super) struct Glitch {
    every: Duration,
    rng: u64,
    pub(super) state: State,
}

impl Glitch {
    /// A timer that glitches on average once per `every`.
    pub(super) fn new(every: Duration, seed: u64) -> Self {
        Self {
            every,
            // xorshift needs a seed other than zero.
            rng: seed.max(1),
            state: State::Off,
        }
    }

    /// The card has just been drawn at `now`: wait for the first glitch.
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
                self.state = State::Tearing(now + length);
                Step::Tear(self.tear())
            }
            State::Tearing(until) if now >= until => {
                self.state = State::Waiting(now + self.interval());
                Step::Settle
            }
            _ => Step::Nothing,
        }
    }

    /// The time to the next glitch: from half to one and a half times
    /// `every`, so the mean is `every`.
    fn interval(&mut self) -> Duration {
        self.every.mul_f64(0.5 + self.unit())
    }

    fn tear(&mut self) -> Tear {
        let mut strips = [0; STRIPS];
        for strip in &mut strips {
            // Most strips jump a little, some a lot.
            let reach = if self.unit() < 0.4 { 90.0 } else { 24.0 };
            *strip = ((self.unit() * 2.0 - 1.0) * reach).round() as i32;
        }
        let chroma = 18 + (self.unit() * 30.0) as u32;
        Tear { strips, chroma }
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
    fn glitches_come_at_random_about_every_interval_and_settle_quickly() {
        let every = Duration::from_secs(20);
        let mut glitch = Glitch::new(every, SEED);
        let start = Instant::now();
        let mut now = start;
        glitch.arm(now);
        let mut tears = Vec::new();
        let mut waits = Vec::new();
        let mut torn_since = None;
        // Step through ten simulated minutes, 10 ms at a time.
        while now < start + Duration::from_secs(600) {
            match glitch.step(now) {
                Step::Nothing => {}
                Step::Tear(tear) => {
                    torn_since = Some(now);
                    tears.push(tear);
                }
                Step::Settle => {
                    let torn = now - torn_since.take().unwrap();
                    assert!(torn >= SHORTEST && torn <= LONGEST + Duration::from_millis(10));
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
        assert!((20..=40).contains(&tears.len()), "{}", tears.len());
        for tear in &tears {
            assert!(tear.strips.iter().all(|s| s.abs() <= 90), "{tear:?}");
            assert!((18..48).contains(&tear.chroma), "{tear:?}");
        }
        assert!(tears.windows(2).any(|w| w[0] != w[1]));
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
