//! The TV's Picture menu: the settings a viewer can change with the remote.
//!
//! There is one setting so far, the CRT filter. The remote has no left or
//! right keys, so OK steps the focused setting to its next value, round in a
//! loop. A choice is stored in the store's viewer settings and outlives a
//! restart; until the viewer chooses, the environment's default applies.

use crate::paint::Crt;
use crate::paint::menu::MenuView;

/// The viewer setting that holds the CRT filter.
pub const CRT_SETTING: &str = "crt";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Menu {
    /// Whether the menu needs drawing again.
    pub(super) stale: bool,
}

impl Menu {
    pub(super) fn new() -> Self {
        Self { stale: true }
    }

    /// What the menu shows with the CRT filter at `crt`.
    pub(super) fn view(&self, crt: Crt) -> MenuView {
        MenuView {
            title: "PICTURE".into(),
            rows: vec![("CRT filter".into(), crt_name(crt).into())],
            focus: 0,
            help: "Scanlines and glow on the guide, the banner and the watermark. Never on the program.".into(),
            keys: "OK CHANGE   BACK CLOSE".into(),
        }
    }
}

/// The CRT level after `crt`: Heavy, Light, Off, then Heavy again.
pub(super) fn next_crt(crt: Crt) -> Crt {
    match crt {
        Crt::Heavy => Crt::Light,
        Crt::Light => Crt::Off,
        Crt::Off => Crt::Heavy,
    }
}

/// The name of a CRT level, as the menu shows it and the store keeps it
/// (in lower case).
pub(super) fn crt_name(crt: Crt) -> &'static str {
    match crt {
        Crt::Heavy => "HEAVY",
        Crt::Light => "LIGHT",
        Crt::Off => "OFF",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_steps_the_crt_filter_round_in_a_loop() {
        assert_eq!(next_crt(Crt::Heavy), Crt::Light);
        assert_eq!(next_crt(Crt::Light), Crt::Off);
        assert_eq!(next_crt(Crt::Off), Crt::Heavy);
        for crt in [Crt::Heavy, Crt::Light, Crt::Off] {
            assert_eq!(
                Crt::parse(crt_name(crt)),
                Some(crt),
                "the store's name parses back"
            );
        }
    }

    #[test]
    fn the_menu_shows_the_current_level() {
        let view = Menu::new().view(Crt::Light);
        assert_eq!(view.rows, [("CRT filter".to_string(), "LIGHT".to_string())]);
    }
}
