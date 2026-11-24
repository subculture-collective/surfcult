//! Keys from the TV remote and keyboards, as the run loop sees them.
//!
//! [`Input`] is what the loop reads: the [`Key`]s pressed since the last
//! poll. [`EvdevInput`] reads them from the Linux input devices with no extra
//! crate: the Pi's CEC remote (a device whose name contains `vc4-hdmi`) and
//! every keyboard. [`FakeInput`] hands out keys a test pushed.
//!
//! | Key | Linux key codes |
//! | --- | --- |
//! | [`Key::Next`] | `KEY_UP` (103), `KEY_CHANNELUP` (402) |
//! | [`Key::Previous`] | `KEY_DOWN` (108), `KEY_CHANNELDOWN` (403) |
//! | [`Key::Banner`] | `KEY_OK` (352), `KEY_ENTER` (28), `KEY_I` (23) |
//! | [`Key::Quit`] | `KEY_Q` (16), from a keyboard only |
//!
//! Only a press counts. A release and the kernel's auto-repeat are ignored,
//! so holding a key changes the channel once.

mod evdev;

use std::collections::VecDeque;

pub use evdev::{
    Device, EVENT_SIZE, EvdevInput, Origin, RawEvent, key_for, parse_devices, parse_events,
};

/// What a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// The next channel up, wrapping from the highest to the lowest.
    Next,
    /// The next channel down, wrapping from the lowest to the highest.
    Previous,
    /// Show the channel banner again.
    Banner,
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
