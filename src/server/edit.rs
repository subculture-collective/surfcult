//! Editing a channel's settings from the web admin: `GET` and
//! `PUT /channels/{number}`.
//!
//! Only the admin inside `surfcult run` edits, because only it has a TV to
//! follow the change. A `PUT` reads `channels.toml`, changes the one
//! channel's keys ([`config::edit`](crate::config::edit)), checks the result
//! as `surfcult run` would at startup and, if it passes, replaces the file in
//! one rename and hands the new config to the TV, which regenerates the
//! changed channels. A file that no longer matches the running config in
//! anything but channel settings (a pool, the time zone, a channel added by
//! hand) is left alone: that needs a restart.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, PoisonError};

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{self, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::control::send;
use super::{AdminCommand, ApiError, AppState, Control};
use crate::config::Config;
use crate::config::edit::{self as config_edit, ChannelSettings, EditError, SOURCE_KINDS};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct EditBody {
    pub(super) number: u32,
    pub(super) settings: ChannelSettings,
    /// The interstitial pools, for `breaks` and `filler`.
    pub(super) pools: Vec<PoolBody>,
    /// The source kinds `settings.source.kind` can be.
    pub(super) source_kinds: Vec<String>,
}

/// An interstitial pool's name and its `kind`, such as `commercial` or
/// `bump`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PoolBody {
    pub(super) name: String,
    pub(super) kind: String,
}

/// `GET /channels/{number}`: the channel's settings as the file writes them.
pub(super) async fn read(
    State(state): State<AppState>,
    number: Result<extract::Path<u32>, PathRejection>,
) -> Result<Json<EditBody>, ApiError> {
    let extract::Path(number) = number.map_err(|r| ApiError::new(r.status(), r.body_text()))?;
    let path = control(&state)?.config_path.clone();
    let settings = blocking(move || {
        config_edit::channel_settings(&read_file(&path)?, number).map_err(edit_error)
    })
    .await?;
    Ok(Json(body(&state, number, settings)))
}

/// `PUT /channels/{number}` with [`ChannelSettings`]: rewrites the channel in
/// `channels.toml` and has the TV follow it. Answers with the settings as
/// written.
pub(super) async fn write(
    State(state): State<AppState>,
    number: Result<extract::Path<u32>, PathRejection>,
    settings: Result<Json<ChannelSettings>, JsonRejection>,
) -> Result<Json<EditBody>, ApiError> {
    let extract::Path(number) = number.map_err(|r| ApiError::new(r.status(), r.body_text()))?;
    let Json(settings) = settings.map_err(|r| ApiError::new(r.status(), r.body_text()))?;
    let control = control(&state)?.clone();
    let shared = state.clone();
    let saved = blocking(move || {
        let _writing = control
            .writing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let text = read_file(&control.config_path)?;
        let new_text = config_edit::set_channel(&text, number, &settings).map_err(edit_error)?;
        let config = Config::parse(&new_text).map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("that would make channels.toml invalid: {e}"),
            )
        })?;
        if !only_settings_differ(&shared.config(), &config) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "channels.toml has changed outside the admin since surfcult started; restart surfcult to load it",
            ));
        }
        if new_text != text {
            replace_file(&control.config_path, &new_text).map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("cannot write channels.toml: {e}"),
                )
            })?;
        }
        *shared
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(config.clone());
        send(&shared, AdminCommand::Reconfigure(Box::new(config)))?;
        config_edit::channel_settings(&new_text, number).map_err(edit_error)
    })
    .await?;
    Ok(Json(body(&state, number, saved)))
}

fn control(state: &AppState) -> Result<&Control, ApiError> {
    state.control.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "this server has no TV; the admin inside surfcult run edits channels",
        )
    })
}

fn body(state: &AppState, number: u32, settings: ChannelSettings) -> EditBody {
    EditBody {
        number,
        settings,
        pools: state
            .config()
            .pools
            .iter()
            .map(|pool| PoolBody {
                name: pool.name.clone(),
                kind: pool.kind.name().to_string(),
            })
            .collect(),
        source_kinds: SOURCE_KINDS.iter().map(|kind| kind.to_string()).collect(),
    }
}

/// Whether `new` has the running config's channels, in the same order, and
/// the same everything else: then the TV can take it without a restart.
fn only_settings_differ(running: &Config, new: &Config) -> bool {
    let numbers =
        |config: &Config| -> Vec<u32> { config.channels.iter().map(|c| c.number).collect() };
    let mut rest = new.clone();
    rest.channels.clone_from(&running.channels);
    numbers(running) == numbers(new) && rest == *running
}

fn read_file(path: &Path) -> Result<String, ApiError> {
    fs::read_to_string(path).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot read channels.toml: {e}"),
        )
    })
}

/// Writes `text` to a new file beside `path` and renames it over `path`, so
/// a crash leaves the old file or the new one, never half of one. A symlink
/// is followed, so it stays a symlink. The new file keeps the old one's
/// permissions.
fn replace_file(path: &Path, text: &str) -> io::Result<()> {
    let path = fs::canonicalize(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.surfcult-new"));
    let written = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(fs::metadata(&path)?.permissions())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    // The rename itself is on disk once the directory is.
    File::open(dir)?.sync_all()
}

fn edit_error(error: EditError) -> ApiError {
    let status = match error {
        EditError::NoChannel(_) => StatusCode::NOT_FOUND,
        // The file was valid when surfcult started.
        EditError::Unreadable(_) => StatusCode::CONFLICT,
        EditError::Refused(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    ApiError::new(status, error.to_string())
}

/// Runs `work` off the async threads: it reads and writes files.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|_| {
        Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the edit did not finish",
        ))
    })
}
