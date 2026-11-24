//! `surfcult run`: the TV on the Pi. Builds everything [`Tv`] needs from
//! the environment and runs its loop until a quit key or a signal.
//!
//! Startup order:
//!
//! 1. load `channels.toml` and open the store read-write (migrating it);
//! 2. scan the clip pools with ffprobe;
//! 3. read the Jellyfin settings from the environment and start the sync
//!    worker, which fetches the library at once and then every
//!    `SURFCULT_SYNC_MINUTES` (15);
//! 4. start MPV with its runtime directory in `$XDG_RUNTIME_DIR/surfcult`
//!    (or `/run/user/<uid>/surfcult`), mode 700;
//! 5. show the stand-by card, extend the schedules (when the clock is
//!    trusted) and tune the lowest channel;
//! 6. loop: apply a finished sync, then [`Tv::step`], every 20 ms.
//!
//! SIGINT, SIGTERM and SIGHUP set a flag the loop checks, so a Ctrl-C, a
//! closed SSH session or `systemctl stop` stops the transcodes and MPV
//! before surfcult exits. MPV is in the same process group and cgroup, so
//! it receives the same signal and may exit first; that is harmless.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Settings, Tv, clock_trusted, jellyfin_streams};
use crate::clips::{ClipLibrary, FfprobeProber};
use crate::config::Config;
use crate::input::EvdevInput;
use crate::jellyfin::{ApiKey, JellyfinClient, JellyfinConfig, SyncWorker};
use crate::player::mpv::{MpvConfig, MpvPlayer};
use crate::store::Store;

/// The Jellyfin server's address.
pub const URL_ENV: &str = "SURFCULT_JELLYFIN_URL";
/// Minutes between two library syncs.
pub const SYNC_ENV: &str = "SURFCULT_SYNC_MINUTES";
/// Seconds the banner stays up.
pub const BANNER_ENV: &str = "SURFCULT_BANNER_SECONDS";
/// The environment variable that names MPV's audio device, such as
/// `alsa/plughw:CARD=vc4hdmi1,DEV=0`. The default is the first HDMI port.
pub const AUDIO_DEVICE_ENV: &str = "SURFCULT_AUDIO_DEVICE";

const DEFAULT_SYNC_MINUTES: u64 = 15;

/// How often the loop runs. The stand-by card's glitch needs a poll at
/// least every 50 ms.
const TICK: Duration = Duration::from_millis(20);

/// Runs the TV with the config at `config_path` and the database at
/// `db_path` until a quit key, SIGINT or SIGTERM.
pub fn run(config_path: &Path, db_path: &Path) -> Result<(), String> {
    let config = Config::load(config_path)
        .map_err(|e| format!("{}: invalid\n{e}", config_path.display()))?;
    let mut store = Store::open(db_path).map_err(|e| format!("{}: {e}", db_path.display()))?;
    let jellyfin = jellyfin_config()?;
    let settings = settings()?;
    let sync_every = sync_every()?;

    let clips = ClipLibrary::scan(&config.pools, FfprobeProber::new());
    for issue in clips.issues() {
        eprintln!("surfcult: clip skipped: {issue}");
    }

    let worker = SyncWorker::spawn(JellyfinClient::new(jellyfin.clone()), sync_every)
        .map_err(|e| format!("cannot start the sync thread: {e}"))?;
    let (resolver, transcodes) = jellyfin_streams(&jellyfin);
    let mut mpv = MpvConfig::new(runtime_dir()?).with_display(&config.display);
    if let Some(device) = std::env::var(AUDIO_DEVICE_ENV)
        .ok()
        .filter(|device| !device.trim().is_empty())
    {
        // A later flag wins over the default in `PI_OUTPUT_ARGS`.
        mpv.output_args
            .push(format!("--audio-device={}", device.trim()));
    }
    let player = MpvPlayer::start(mpv, resolver).map_err(|e| e.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, stop.clone())
            .map_err(|e| format!("cannot handle signal {signal}: {e}"))?;
    }

    let mut tv = Tv::new(
        config,
        player,
        EvdevInput::new(),
        clips,
        Box::new(transcodes),
        Box::new(clock_trusted),
        settings,
    );
    tv.start(now_ms(), &mut store);
    while !stop.load(Ordering::Relaxed) && !tv.quit_requested() {
        let now = now_ms();
        if let Some(result) = worker.apply(&mut store, now) {
            tv.synced(now, &mut store, result);
        }
        tv.step(now, &mut store);
        std::thread::sleep(TICK);
    }
    eprintln!("surfcult: stopping");
    tv.shut_down();
    Ok(())
}

/// The system clock in Unix epoch milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn jellyfin_config() -> Result<JellyfinConfig, String> {
    let base_url = std::env::var(URL_ENV)
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            format!(
                "{URL_ENV} is not set; it is the Jellyfin address, such as http://10.0.0.200:8096"
            )
        })?;
    let key = ApiKey::from_env().map_err(|e| e.to_string())?;
    Ok(JellyfinConfig {
        base_url,
        key,
        user_id: JellyfinConfig::user_id_from_env(),
    })
}

fn settings() -> Result<Settings, String> {
    let mut settings = Settings::default();
    if let Some(seconds) = positive_env(BANNER_ENV)? {
        settings.banner_ms = i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    }
    Ok(settings)
}

fn sync_every() -> Result<Duration, String> {
    let minutes = positive_env(SYNC_ENV)?.unwrap_or(DEFAULT_SYNC_MINUTES);
    Ok(Duration::from_secs(minutes.saturating_mul(60)))
}

/// A whole number above 0 from `name`, if it is set.
fn positive_env(name: &str) -> Result<Option<u64>, String> {
    match std::env::var(name) {
        Err(_) => Ok(None),
        Ok(text) => match text.trim().parse::<u64>() {
            Ok(n) if n > 0 => Ok(Some(n)),
            _ => Err(format!(
                "{name} must be a whole number above 0, not {text:?}"
            )),
        },
    }
}

/// `$XDG_RUNTIME_DIR/surfcult`, or `/run/user/<uid>/surfcult` without it.
/// The player creates it with mode 700.
fn runtime_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("surfcult"));
    }
    // `/proc/self` belongs to the process's user.
    let uid = std::fs::metadata("/proc/self")
        .map_err(|e| format!("cannot find the user id for the runtime directory: {e}"))?
        .uid();
    Ok(PathBuf::from(format!("/run/user/{uid}/surfcult")))
}
