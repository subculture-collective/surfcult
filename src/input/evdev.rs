//! Linux input devices read with `std` alone.
//!
//! The devices are listed in `/proc/bus/input/devices`. A device is read
//! when its name contains `vc4-hdmi` (the Pi's CEC remote; its event number
//! changes between boots, so it is found by name) or when it has the `kbd`
//! handler (a keyboard). Each one is opened non-blocking as
//! `/dev/input/eventN` and read as a stream of `struct input_event`: on a
//! 64-bit system a 16-byte `timeval`, then `type` (`u16`), `code` (`u16`) and
//! `value` (`i32`), 24 bytes in native byte order. The device list is read
//! again every few seconds, so a remote or keyboard that appears later is
//! picked up and one that goes away is dropped.
//!
//! Reading needs read access to the device files: on the Pi the user is in
//! the `input` group. The devices are not grabbed, so the console also sees
//! the keys; nothing reads them there.

use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use super::{Input, Key};

/// The size of one `struct input_event`.
#[cfg(target_pointer_width = "64")]
pub const EVENT_SIZE: usize = 24;
/// The size of one `struct input_event`.
#[cfg(not(target_pointer_width = "64"))]
pub const EVENT_SIZE: usize = 16;

/// Bytes before `type`: the `timeval`.
const TIME_SIZE: usize = EVENT_SIZE - 8;

/// `EV_KEY`, the event type of a key.
const EV_KEY: u16 = 1;

/// `value` of a key event: 0 release, 1 press, 2 auto-repeat.
const PRESS: i32 = 1;

/// Linux's `O_NONBLOCK`, the same on every architecture the Pi and a
/// development machine run.
const O_NONBLOCK: i32 = 0o4000;

/// How often the device list is read again, for hotplug.
const RESCAN_MS: i64 = 3_000;

const KEY_ENTER: u16 = 28;
const KEY_I: u16 = 23;
const KEY_Q: u16 = 16;
const KEY_UP: u16 = 103;
const KEY_DOWN: u16 = 108;
const KEY_OK: u16 = 352;
const KEY_CHANNELUP: u16 = 402;
const KEY_CHANNELDOWN: u16 = 403;

/// One `struct input_event`, without its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// Whether a device is the remote or a keyboard. Only a keyboard can quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Remote,
    Keyboard,
}

/// One entry of `/proc/bus/input/devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The `N: Name="..."` value.
    pub name: String,
    /// The `H: Handlers=...` words, such as `kbd` and `event0`.
    pub handlers: Vec<String>,
}

impl Device {
    /// The `eventN` handler, the node under `/dev/input`.
    pub fn event_node(&self) -> Option<&str> {
        self.handlers
            .iter()
            .map(String::as_str)
            .find(|h| h.strip_prefix("event").is_some_and(is_number))
    }

    /// Whether surfcult reads this device, and as what.
    pub fn origin(&self) -> Option<Origin> {
        if self.name.contains("vc4-hdmi") {
            Some(Origin::Remote)
        } else if self.handlers.iter().any(|h| h == "kbd") {
            Some(Origin::Keyboard)
        } else {
            None
        }
    }
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The devices in the text of `/proc/bus/input/devices`: blocks separated by
/// blank lines, each with an `N:` name line and an `H:` handlers line.
pub fn parse_devices(text: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    let mut name = None;
    let mut handlers = Vec::new();
    for line in text.lines().chain([""]) {
        if line.trim().is_empty() {
            if let Some(name) = name.take() {
                devices.push(Device {
                    name,
                    handlers: std::mem::take(&mut handlers),
                });
            }
            handlers.clear();
        } else if let Some(rest) = line.strip_prefix("N: Name=") {
            name = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
            handlers = rest.split_whitespace().map(str::to_string).collect();
        }
    }
    devices
}

/// The whole events in `bytes`, and how many bytes they took. A trailing
/// partial event is left for the next read.
pub fn parse_events(bytes: &[u8]) -> (Vec<RawEvent>, usize) {
    let events: Vec<RawEvent> = bytes
        .as_chunks::<EVENT_SIZE>()
        .0
        .iter()
        .map(|event| {
            let field = &event[TIME_SIZE..];
            RawEvent {
                kind: u16::from_ne_bytes([field[0], field[1]]),
                code: u16::from_ne_bytes([field[2], field[3]]),
                value: i32::from_ne_bytes([field[4], field[5], field[6], field[7]]),
            }
        })
        .collect();
    let used = events.len() * EVENT_SIZE;
    (events, used)
}

/// The [`Key`] an event from a device of `origin` means, if any. Only key
/// presses count; releases and auto-repeats do not.
pub fn key_for(origin: Origin, event: RawEvent) -> Option<Key> {
    if event.kind != EV_KEY || event.value != PRESS {
        return None;
    }
    match event.code {
        KEY_UP | KEY_CHANNELUP => Some(Key::Next),
        KEY_DOWN | KEY_CHANNELDOWN => Some(Key::Previous),
        KEY_OK | KEY_ENTER | KEY_I => Some(Key::Banner),
        KEY_Q if origin == Origin::Keyboard => Some(Key::Quit),
        _ => None,
    }
}

struct Open {
    file: File,
    origin: Origin,
    name: String,
    pending: Vec<u8>,
}

/// The remote and keyboards, read from `/dev/input`. See the module
/// documentation.
pub struct EvdevInput {
    devices_file: PathBuf,
    dev_dir: PathBuf,
    /// Open devices by node name (`event0`).
    open: BTreeMap<String, Open>,
    /// Nodes that could not be opened, so the error is logged once.
    refused: HashSet<String>,
    next_scan_ms: i64,
}

impl EvdevInput {
    /// Reads `/proc/bus/input/devices` and `/dev/input`. Devices are opened
    /// on the first poll.
    pub fn new() -> Self {
        Self::at("/proc/bus/input/devices", "/dev/input")
    }

    /// The same with other paths, for tests.
    pub fn at(devices_file: impl Into<PathBuf>, dev_dir: impl Into<PathBuf>) -> Self {
        Self {
            devices_file: devices_file.into(),
            dev_dir: dev_dir.into(),
            open: BTreeMap::new(),
            refused: HashSet::new(),
            next_scan_ms: i64::MIN,
        }
    }

    /// The names of the devices being read, by node.
    pub fn devices(&self) -> Vec<(&str, &str)> {
        self.open
            .iter()
            .map(|(node, open)| (node.as_str(), open.name.as_str()))
            .collect()
    }

    /// Opens the devices that appeared since the last scan.
    fn rescan(&mut self) {
        let text = match std::fs::read_to_string(&self.devices_file) {
            Ok(text) => text,
            Err(e) => {
                if self.refused.insert(String::new()) {
                    eprintln!(
                        "surfcult: cannot list input devices in {}: {e}",
                        self.devices_file.display()
                    );
                }
                return;
            }
        };
        let listed = parse_devices(&text);
        let nodes: HashSet<&str> = listed.iter().filter_map(Device::event_node).collect();
        self.refused.retain(|node| nodes.contains(node.as_str()));
        for device in &listed {
            let (Some(node), Some(origin)) = (device.event_node(), device.origin()) else {
                continue;
            };
            if self.open.contains_key(node) || self.refused.contains(node) {
                continue;
            }
            let path = self.dev_dir.join(node);
            match OpenOptions::new()
                .read(true)
                .custom_flags(O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => {
                    eprintln!(
                        "surfcult: reading keys from {:?} ({})",
                        device.name,
                        path.display()
                    );
                    self.open.insert(
                        node.to_string(),
                        Open {
                            file,
                            origin,
                            name: device.name.clone(),
                            pending: Vec::new(),
                        },
                    );
                }
                Err(e) => {
                    eprintln!(
                        "surfcult: cannot read keys from {:?} ({}): {e}",
                        device.name,
                        path.display()
                    );
                    self.refused.insert(node.to_string());
                }
            }
        }
    }
}

impl Default for EvdevInput {
    fn default() -> Self {
        Self::new()
    }
}

impl Input for EvdevInput {
    fn poll(&mut self, now_ms: i64) -> Vec<Key> {
        if now_ms >= self.next_scan_ms {
            self.next_scan_ms = now_ms.saturating_add(RESCAN_MS);
            self.rescan();
        }
        let mut keys = Vec::new();
        let mut gone = Vec::new();
        for (node, open) in &mut self.open {
            let mut chunk = [0; EVENT_SIZE * 32];
            loop {
                match open.file.read(&mut chunk) {
                    Ok(0) => {
                        gone.push((node.clone(), "end of file".to_string()));
                        break;
                    }
                    Ok(n) => open.pending.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) => {
                        gone.push((node.clone(), e.to_string()));
                        break;
                    }
                }
            }
            let (events, used) = parse_events(&open.pending);
            open.pending.drain(..used);
            keys.extend(events.into_iter().filter_map(|e| key_for(open.origin, e)));
        }
        for (node, why) in gone {
            if let Some(open) = self.open.remove(&node) {
                eprintln!("surfcult: stopped reading keys from {:?}: {why}", open.name);
            }
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = r#"I: Bus=0019 Vendor=0000 Product=0000 Version=0000
N: Name="vc4-hdmi-0"
P: Phys=vc4-hdmi-0/input0
S: Sysfs=/devices/platform/soc/fef00700.hdmi/rc/rc0/input0
U: Uniq=
H: Handlers=kbd event2 
B: PROP=20
B: EV=100017

I: Bus=0003 Vendor=046d Product=c31c Version=0110
N: Name="Logitech USB Keyboard"
P: Phys=usb-0000:01:00.0-1.3/input0
S: Sysfs=/devices/platform/scb/fd500000.pcie/usb1/1-1/1-1.3/input/input1
U: Uniq=
H: Handlers=sysrq kbd leds event0 
B: PROP=0
B: EV=120013

I: Bus=0003 Vendor=046d Product=c077 Version=0111
N: Name="Logitech USB Optical Mouse"
H: Handlers=mouse0 event1 
B: EV=17

I: Bus=0019 Vendor=0000 Product=0000 Version=0000
N: Name="vc4-hdmi-1"
H: Handlers=event3
"#;

    #[test]
    fn finds_the_remote_by_name_and_keyboards_by_handler() {
        let devices = parse_devices(DEVICES);
        let found: Vec<_> = devices
            .iter()
            .map(|d| (d.name.as_str(), d.event_node(), d.origin()))
            .collect();
        assert_eq!(
            found,
            [
                ("vc4-hdmi-0", Some("event2"), Some(Origin::Remote)),
                (
                    "Logitech USB Keyboard",
                    Some("event0"),
                    Some(Origin::Keyboard)
                ),
                ("Logitech USB Optical Mouse", Some("event1"), None),
                ("vc4-hdmi-1", Some("event3"), Some(Origin::Remote)),
            ]
        );
    }

    fn raw(kind: u16, code: u16, value: i32) -> Vec<u8> {
        let mut bytes = vec![0xAB; TIME_SIZE];
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&code.to_ne_bytes());
        bytes.extend_from_slice(&value.to_ne_bytes());
        bytes
    }

    #[test]
    fn parses_whole_events_and_leaves_a_partial_one() {
        let mut bytes = raw(EV_KEY, KEY_UP, 1);
        bytes.extend(raw(0, 0, 0));
        bytes.extend(&raw(EV_KEY, KEY_OK, 0)[..10]);
        let (events, used) = parse_events(&bytes);
        assert_eq!(used, 2 * EVENT_SIZE);
        assert_eq!(
            events,
            [
                RawEvent {
                    kind: EV_KEY,
                    code: KEY_UP,
                    value: 1
                },
                RawEvent {
                    kind: 0,
                    code: 0,
                    value: 0
                },
            ]
        );
    }

    #[test]
    fn only_a_press_counts() {
        let press = |code| RawEvent {
            kind: EV_KEY,
            code,
            value: PRESS,
        };
        let remote = |code| key_for(Origin::Remote, press(code));
        assert_eq!(remote(KEY_UP), Some(Key::Next));
        assert_eq!(remote(KEY_CHANNELUP), Some(Key::Next));
        assert_eq!(remote(KEY_DOWN), Some(Key::Previous));
        assert_eq!(remote(KEY_CHANNELDOWN), Some(Key::Previous));
        assert_eq!(remote(KEY_OK), Some(Key::Banner));
        assert_eq!(remote(KEY_ENTER), Some(Key::Banner));
        assert_eq!(remote(KEY_I), Some(Key::Banner));
        assert_eq!(remote(105), None); // KEY_LEFT
        // Q quits from a keyboard only.
        assert_eq!(remote(KEY_Q), None);
        assert_eq!(key_for(Origin::Keyboard, press(KEY_Q)), Some(Key::Quit));
        for value in [0, 2] {
            let event = RawEvent {
                kind: EV_KEY,
                code: KEY_UP,
                value,
            };
            assert_eq!(key_for(Origin::Remote, event), None);
        }
        // A key code in another event type (EV_MSC) is not a key.
        let msc = RawEvent {
            kind: 4,
            code: KEY_UP,
            value: 1,
        };
        assert_eq!(key_for(Origin::Remote, msc), None);
    }

    #[test]
    fn reads_listed_devices_and_picks_up_new_ones() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("devices");
        // Regular files stand in for the device nodes; a read at their end
        // returns 0, which drops the device, so each test write is read once.
        std::fs::write(&list, DEVICES).unwrap();
        let mut bytes = raw(EV_KEY, KEY_DOWN, 1);
        bytes.extend(raw(EV_KEY, KEY_DOWN, 2));
        bytes.extend(raw(EV_KEY, KEY_DOWN, 0));
        std::fs::write(dir.path().join("event2"), &bytes).unwrap();
        let mut input = EvdevInput::at(&list, dir.path());
        assert_eq!(input.poll(0), [Key::Previous]);
        // Its file ended, so it was dropped; with the file gone it stays so.
        assert!(input.devices().is_empty());
        std::fs::remove_file(dir.path().join("event2")).unwrap();
        // event0 (the keyboard) and event3 had no file, so they were refused.
        // Once it appears and the list is read again, it is opened.
        std::fs::write(dir.path().join("event0"), raw(EV_KEY, KEY_Q, 1)).unwrap();
        assert_eq!(input.poll(1_000), []);
        let listed = DEVICES.replace("kbd leds event0", "kbd leds event4");
        std::fs::write(&list, listed).unwrap();
        std::fs::rename(dir.path().join("event0"), dir.path().join("event4")).unwrap();
        assert_eq!(input.poll(RESCAN_MS), [Key::Quit]);
    }
}
