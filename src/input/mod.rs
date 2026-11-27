//! Keys from the TV remote and keyboards, as the run loop sees them.
//!
//! [`Input`] is what the loop reads: the [`Key`]s pressed since the last
//! poll. [`EvdevInput`] reads them from the Linux input devices with no extra
//! crate: the Pi's CEC remote (a device whose name contains `vc4-hdmi`) and
//! every keyboard. [`FakeInput`] hands out keys a test pushed.
//!
//! Which Linux key codes mean which [`Key`] is the [`KeyMap`]: the `[keys]`
//! table of `channels.toml` over defaults (see `docs/reference/run.md`).
//! Quit works from a keyboard only.
//!
//! Only a press counts. A release and the kernel's auto-repeat are ignored,
//! so holding a key changes the channel once.

mod evdev;
mod keymap;

use std::collections::VecDeque;

pub use evdev::{
    Device, EVENT_SIZE, EvdevInput, Origin, RawEvent, key_for, parse_devices, parse_events,
};
pub use keymap::{KeyMap, KeyProblem, code_for};

/// What a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// The next channel up, wrapping from the highest to the lowest.
    Next,
    /// The next channel down, wrapping from the lowest to the highest.
    Previous,
    /// Show the channel banner again.
    Banner,
    /// A digit of a channel number being entered.
    Digit(u8),
    /// Go back: closes the guide or the menu.
    Back,
    /// Open or close the Picture menu.
    Menu,
    /// Open or close the guide.
    Guide,
    /// Mute. The run loop ignores it for now.
    Mute,
    /// Volume up. The run loop ignores it for now.
    VolumeUp,
    /// Volume down. The run loop ignores it for now.
    VolumeDown,
    /// Stop surfcult.
    Quit,
}

/// A source of key presses. `poll` never waits.
pub trait Input {
    /// The keys pressed since the last call, oldest first. `now_ms` is the
    /// loop's clock, for an input that does something on a timer.
    fn poll(&mut self, now_ms: i64) -> Vec<Key>;
}

/// An [`Input`] for tests: it returns the keys pushed since the last poll.
#[derive(Debug, Default)]
pub struct FakeInput {
    keys: VecDeque<Key>,
}

impl FakeInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: Key) {
        self.keys.push_back(key);
    }
}

impl Input for FakeInput {
    fn poll(&mut self, _now_ms: i64) -> Vec<Key> {
        self.keys.drain(..).collect()
    }
}
