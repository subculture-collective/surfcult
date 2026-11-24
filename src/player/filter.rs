//! Display filters: named effects the player applies to the picture.

use std::fmt;
use std::time::Duration;

/// A named effect on the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// A GLSL shader that makes the picture look like a CRT television:
    /// light scanlines, slight curvature with a soft vignette, red and cyan fringing
    /// toward the edges, a faint flicker and a slow rolling band.
    Crt,
    /// An occasional flicker of the stand-by card's band: one or two thin
    /// slices move a few pixels sideways and the red and cyan copies of the
    /// text move a little further apart, for 80 to 150 ms.
    Glitch,
}

impl Filter {
    /// Every filter, in the order their names are listed in messages.
    pub const ALL: [Filter; 2] = [Filter::Crt, Filter::Glitch];

    /// The filter with this name in `channels.toml`, if there is one.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The filter's name in `channels.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Crt => "crt",
            Self::Glitch => "glitch",
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The filters the stand-by card applies when nothing is configured.
pub const DEFAULT_STANDBY_FILTERS: [Filter; 2] = [Filter::Crt, Filter::Glitch];

/// The mean time between two glitches when nothing is configured.
pub const DEFAULT_GLITCH_EVERY: Duration = Duration::from_secs(20);

/// The filters on screen while the stand-by card shows: `global` followed by
/// `standby`, each filter at most once, so a filter that is on everywhere is
/// not applied a second time on the card.
pub fn standby_filters(global: &[Filter], standby: &[Filter]) -> Vec<Filter> {
    let mut all = Vec::new();
    for &filter in global.iter().chain(standby) {
        if !all.contains(&filter) {
            all.push(filter);
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use Filter::{Crt, Glitch};

    #[test]
    fn names_round_trip() {
        for filter in Filter::ALL {
            assert_eq!(Filter::from_name(filter.name()), Some(filter));
        }
        assert_eq!(Filter::from_name("CRT"), None);
        assert_eq!(Filter::from_name("vhs"), None);
    }

    #[test]
    fn the_card_gets_the_global_filters_then_its_own_each_once() {
        // Global only.
        assert_eq!(standby_filters(&[Crt], &[]), [Crt]);
        // Stand-by only.
        assert_eq!(standby_filters(&[], &[Glitch, Crt]), [Glitch, Crt]);
        // Overlap: the global CRT is not added a second time.
        assert_eq!(standby_filters(&[Crt], &[Crt, Glitch]), [Crt, Glitch]);
        // The glitch in both lists runs once.
        assert_eq!(standby_filters(&[Glitch], &[Glitch]), [Glitch]);
        assert_eq!(
            standby_filters(&[Glitch, Crt, Glitch], &[Crt, Glitch]),
            [Glitch, Crt]
        );
        assert_eq!(standby_filters(&[], &[]), []);
    }
}
