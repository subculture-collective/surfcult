//! The key map: which Linux key codes mean which [`Key`].
//!
//! The map is the `[keys]` table of `channels.toml` over the defaults. Each
//! action lists key names (`KEY_UP`, as in `linux/input-event-codes.h`).
//! An action the table names replaces that action's default keys, and a key it
//! gives to an action is taken from any default action that had it.

use std::collections::{BTreeMap, HashMap};

use super::Key;

/// One problem with the table: the field (`keys.guide[1]`) and what is wrong.
pub type KeyProblem = (String, String);

/// The actions other than the digits, and their default keys.
const DEFAULTS: &[(&str, Key, &[&str])] = &[
    (
        "channel_up",
        Key::Next,
        &["KEY_UP", "KEY_CHANNELUP", "KEY_PAGEUP"],
    ),
    (
        "channel_down",
        Key::Previous,
        &["KEY_DOWN", "KEY_CHANNELDOWN", "KEY_PAGEDOWN"],
    ),
    (
        "info",
        Key::Banner,
        &["KEY_OK", "KEY_ENTER", "KEY_INFO", "KEY_I"],
    ),
    (
        "back",
        Key::Back,
        &["KEY_EXIT", "KEY_BACK", "KEY_ESC", "KEY_BACKSPACE"],
    ),
    ("menu", Key::Menu, &["KEY_MENU", "KEY_M"]),
    (
        "guide",
        Key::Guide,
        &[
            "KEY_EPG",
            "KEY_PROGRAM",
            "KEY_G",
            "KEY_PLAYCD",
            "KEY_PAUSECD",
            "KEY_PLAYPAUSE",
        ],
    ),
    ("mute", Key::Mute, &["KEY_MUTE"]),
    ("volume_up", Key::VolumeUp, &["KEY_VOLUMEUP"]),
    ("volume_down", Key::VolumeDown, &["KEY_VOLUMEDOWN"]),
    ("quit", Key::Quit, &["KEY_Q"]),
];

const DIGIT_NAMES: [&str; 10] = [
    "digit_0", "digit_1", "digit_2", "digit_3", "digit_4", "digit_5", "digit_6", "digit_7",
    "digit_8", "digit_9",
];

/// The action named `name`, and its default key names.
fn action(name: &str) -> Option<(Key, Vec<String>)> {
    if let Some(d) = DIGIT_NAMES.iter().position(|n| *n == name) {
        let names = vec![
            format!("KEY_{d}"),
            format!("KEY_KP{d}"),
            format!("KEY_NUMERIC_{d}"),
        ];
        return Some((Key::Digit(d as u8), names));
    }
    DEFAULTS
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, key, names)| (*key, names.iter().map(|n| n.to_string()).collect()))
}

/// Every action name.
fn action_names() -> Vec<&'static str> {
    DEFAULTS
        .iter()
        .map(|(n, ..)| *n)
        .chain(DIGIT_NAMES)
        .collect()
}

/// Codes of the keys that are not a letter or a digit.
const NAMED: &[(&str, u16)] = &[
    ("KEY_ESC", 1),
    ("KEY_MINUS", 12),
    ("KEY_EQUAL", 13),
    ("KEY_BACKSPACE", 14),
    ("KEY_TAB", 15),
    ("KEY_ENTER", 28),
    ("KEY_SPACE", 57),
    ("KEY_F1", 59),
    ("KEY_F2", 60),
    ("KEY_F3", 61),
    ("KEY_F4", 62),
    ("KEY_F5", 63),
    ("KEY_F6", 64),
    ("KEY_F7", 65),
    ("KEY_F8", 66),
    ("KEY_F9", 67),
    ("KEY_F10", 68),
    ("KEY_F11", 87),
    ("KEY_F12", 88),
    ("KEY_KPENTER", 96),
    ("KEY_HOME", 102),
    ("KEY_UP", 103),
    ("KEY_PAGEUP", 104),
    ("KEY_LEFT", 105),
    ("KEY_RIGHT", 106),
    ("KEY_END", 107),
    ("KEY_DOWN", 108),
    ("KEY_PAGEDOWN", 109),
    ("KEY_MUTE", 113),
    ("KEY_VOLUMEDOWN", 114),
    ("KEY_VOLUMEUP", 115),
    ("KEY_PAUSE", 119),
    ("KEY_MENU", 139),
    ("KEY_BACK", 158),
    ("KEY_PLAYPAUSE", 164),
    ("KEY_REWIND", 168),
    ("KEY_EXIT", 174),
    ("KEY_PLAYCD", 200),
    ("KEY_PAUSECD", 201),
    ("KEY_PLAY", 207),
    ("KEY_FASTFORWARD", 208),
    ("KEY_OK", 352),
    ("KEY_INFO", 358),
    ("KEY_PROGRAM", 362),
    ("KEY_EPG", 365),
    ("KEY_CHANNELUP", 402),
    ("KEY_CHANNELDOWN", 403),
];

/// Letter codes in keyboard order: the top row, the home row, the bottom row.
const LETTERS: [(&str, u16); 3] = [("QWERTYUIOP", 16), ("ASDFGHJKL", 30), ("ZXCVBNM", 44)];

/// The keypad's digits 0 to 9.
const KEYPAD: [u16; 10] = [82, 79, 80, 81, 75, 76, 77, 71, 72, 73];

/// The Linux key code named `name`, such as `KEY_UP`.
pub fn code_for(name: &str) -> Option<u16> {
    let rest = name.strip_prefix("KEY_")?;
    if let Some(&(_, code)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(code);
    }
    let digit = |text: &str| match text.as_bytes() {
        [d @ b'0'..=b'9'] => Some((d - b'0') as usize),
        _ => None,
    };
    if let Some(d) = digit(rest) {
        return Some(if d == 0 { 11 } else { 1 + d as u16 });
    }
    if let Some(d) = rest.strip_prefix("KP").and_then(digit) {
        return Some(KEYPAD[d]);
    }
    if let Some(d) = rest.strip_prefix("NUMERIC_").and_then(digit) {
        return Some(0x200 + d as u16);
    }
    let [letter] = rest.as_bytes() else {
        return None;
    };
    LETTERS.iter().find_map(|(row, first)| {
        row.bytes()
            .position(|b| b == *letter)
            .map(|i| first + i as u16)
    })
}

/// Which [`Key`] each Linux key code means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    codes: HashMap<u16, Key>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::from_table(&BTreeMap::new()).expect("the default keys are valid")
    }
}

impl KeyMap {
    /// The map for a `[keys]` table of action name to key names. Every
    /// problem is returned with its field, so the config can report them.
    pub fn from_table(table: &BTreeMap<String, Vec<String>>) -> Result<Self, Vec<KeyProblem>> {
        let mut problems = Vec::new();
        let mut codes: HashMap<u16, Key> = HashMap::new();
        let mut owner: HashMap<u16, &str> = HashMap::new();
        for (name, keys) in table {
            let Some((key, _)) = action(name) else {
                problems.push((
                    format!("keys.{name}"),
                    format!("unknown action {name:?}; use {}", action_names().join(", ")),
                ));
                continue;
            };
            for (i, text) in keys.iter().enumerate() {
                let Some(code) = code_for(text) else {
                    problems.push((
                        format!("keys.{name}[{i}]"),
                        format!("unknown key {text:?}; use a Linux key name such as KEY_UP"),
                    ));
                    continue;
                };
                match owner.insert(code, name) {
                    Some(first) if first != name => problems.push((
                        format!("keys.{name}[{i}]"),
                        format!("{text} is already used by {first}"),
                    )),
                    _ => {
                        codes.insert(code, key);
                    }
                }
            }
        }
        for name in action_names() {
            if table.contains_key(name) {
                continue;
            }
            let (key, names) = action(name).expect("a listed action");
            for code in names.iter().filter_map(|n| code_for(n)) {
                codes.entry(code).or_insert(key);
            }
        }
        if problems.is_empty() {
            Ok(Self { codes })
        } else {
            Err(problems)
        }
    }

    /// The key a Linux key code means.
    pub fn key(&self, code: u16) -> Option<Key> {
        self.codes.get(&code).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(n, keys)| (n.to_string(), keys.iter().map(|k| k.to_string()).collect()))
            .collect()
    }

    #[test]
    fn names_resolve_to_linux_codes() {
        for (name, code) in [
            ("KEY_UP", 103),
            ("KEY_OK", 352),
            ("KEY_Q", 16),
            ("KEY_A", 30),
            ("KEY_M", 50),
            ("KEY_I", 23),
            ("KEY_0", 11),
            ("KEY_1", 2),
            ("KEY_9", 10),
            ("KEY_KP0", 82),
            ("KEY_KP1", 79),
            ("KEY_KP9", 73),
            ("KEY_NUMERIC_0", 512),
            ("KEY_NUMERIC_9", 521),
            ("KEY_PLAYCD", 200),
            ("KEY_EXIT", 174),
        ] {
            assert_eq!(code_for(name), Some(code), "{name}");
        }
        for name in ["UP", "KEY_", "KEY_AA", "KEY_KP10", "key_up", "KEY_NOPE"] {
            assert_eq!(code_for(name), None, "{name}");
        }
    }

    #[test]
    fn the_defaults_map_the_fire_tv_remote() {
        let map = KeyMap::default();
        assert_eq!(map.key(103), Some(Key::Next));
        assert_eq!(map.key(109), Some(Key::Previous));
        assert_eq!(map.key(352), Some(Key::Banner));
        assert_eq!(map.key(139), Some(Key::Menu));
        assert_eq!(map.key(174), Some(Key::Back));
        assert_eq!(map.key(200), Some(Key::Guide));
        assert_eq!(map.key(201), Some(Key::Guide));
        assert_eq!(map.key(113), Some(Key::Mute));
        assert_eq!(map.key(115), Some(Key::VolumeUp));
        assert_eq!(map.key(114), Some(Key::VolumeDown));
        assert_eq!(map.key(16), Some(Key::Quit));
        assert_eq!(map.key(11), Some(Key::Digit(0)));
        assert_eq!(map.key(5), Some(Key::Digit(4)));
        assert_eq!(map.key(82), Some(Key::Digit(0)));
        assert_eq!(map.key(521), Some(Key::Digit(9)));
        assert_eq!(map.key(105), None); // KEY_LEFT
        assert_eq!(map.key(168), None); // KEY_REWIND has no action yet
    }

    #[test]
    fn a_named_action_replaces_its_defaults_and_takes_keys_from_others() {
        let map = KeyMap::from_table(&table(&[
            ("channel_up", &["KEY_RIGHT", "KEY_G"]),
            ("volume_up", &[]),
        ]))
        .unwrap();
        assert_eq!(map.key(106), Some(Key::Next));
        assert_eq!(map.key(103), None);
        assert_eq!(map.key(34), Some(Key::Next)); // KEY_G, no longer the guide
        assert_eq!(map.key(115), None); // an empty list turns the action off
        assert_eq!(map.key(108), Some(Key::Previous)); // untouched
    }

    #[test]
    fn problems_name_the_field() {
        let problems = KeyMap::from_table(&table(&[
            ("zap", &["KEY_UP"]),
            ("guide", &["KEY_G", "KEY_WAT"]),
            ("menu", &["KEY_G"]),
        ]))
        .unwrap_err();
        let fields: Vec<&str> = problems.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(fields, ["keys.guide[1]", "keys.menu[0]", "keys.zap"]);
    }
}
