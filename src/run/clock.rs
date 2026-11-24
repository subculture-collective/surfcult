//! Whether the system clock can be trusted to write schedules.
//!
//! A Pi has no battery-backed clock: until it synchronizes over the network
//! it may think it is 1970 or the time it was last shut down, and a week
//! generated then would be wrong. `schedule::extend` takes the answer as its
//! `clock_trusted` argument.
//!
//! [`trusted`] answers, in this order:
//!
//! 1. `SURFCULT_CLOCK_TRUSTED=1` says yes and `SURFCULT_CLOCK_TRUSTED=0`
//!    says no, for testing.
//! 2. `timedatectl show -p NTPSynchronized --value` printing `yes`. systemd
//!    reports the kernel's synchronized flag there, which systemd-timesyncd
//!    and chrony both set.
//! 3. If `timedatectl` cannot be run, the file
//!    `/run/systemd/timesync/synchronized`, which systemd-timesyncd creates
//!    once it has synchronized.

use std::path::Path;
use std::process::Command;

/// The environment variable that overrides the check.
pub const CLOCK_ENV: &str = "SURFCULT_CLOCK_TRUSTED";

/// The file systemd-timesyncd creates when it has synchronized.
const TIMESYNC_FILE: &str = "/run/systemd/timesync/synchronized";

/// Whether the clock is synchronized. See the module documentation.
pub fn trusted() -> bool {
    let env = std::env::var(CLOCK_ENV).ok();
    decide(env.as_deref(), timedatectl, || {
        Path::new(TIMESYNC_FILE).exists()
    })
}

/// What `timedatectl` says, or `None` when it cannot be run or fails.
fn timedatectl() -> Option<String> {
    let output = Command::new("timedatectl")
        .args(["show", "-p", "NTPSynchronized", "--value"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn decide(
    env: Option<&str>,
    timedatectl: impl FnOnce() -> Option<String>,
    timesync_file: impl FnOnce() -> bool,
) -> bool {
    match env.map(str::trim) {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    match timedatectl() {
        Some(answer) => answer == "yes",
        None => timesync_file(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_overrides_systemd() {
        let never = || -> Option<String> { panic!("timedatectl was asked") };
        assert!(decide(Some("1"), never, || false));
        assert!(!decide(Some("0"), || Some("yes".into()), || true));
    }

    #[test]
    fn timedatectl_answers_and_the_file_is_the_fallback() {
        assert!(decide(None, || Some("yes".into()), || false));
        assert!(!decide(None, || Some("no".into()), || true));
        assert!(decide(Some(""), || None, || true));
        assert!(!decide(None, || None, || false));
    }
}
