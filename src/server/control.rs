//! Controlling the TV from the web admin, when the server runs inside
//! `surfcult run`.
//!
//! The run loop gives the server a [`Control`]: a channel the server sends
//! [`AdminCommand`]s down, which the loop reads at each step, and a
//! [`TvStatus`] the loop keeps up to date, which the server reads. Standalone
//! `surfcult serve` has no TV, so its control routes answer `503`.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState};
use crate::config::Config;
use crate::paint::Crt;

/// What the web admin asks the TV to do.
#[derive(Debug, Clone, PartialEq)]
pub enum AdminCommand {
    /// Change to this channel, as the remote would.
    Tune(u32),
    /// Use this CRT filter and store it, as the Picture menu would.
    SetCrt(Crt),
    /// `channels.toml` was rewritten with this config, which differs from
    /// the running one only in its channels' settings.
    Reconfigure(Box<Config>),
}

/// What the TV is doing, as the run loop last wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TvStatus {
    pub channel: Option<u32>,
    pub crt: Crt,
}

/// The server's line to the TV.
#[derive(Clone)]
pub struct Control {
    commands: Sender<AdminCommand>,
    status: Arc<Mutex<TvStatus>>,
    /// The `channels.toml` the TV runs, which the admin rewrites.
    pub(super) config_path: PathBuf,
    /// Held while `channels.toml` is read, changed and written, so two edits
    /// cannot lose one another.
    pub(super) writing: Arc<Mutex<()>>,
}

impl Control {
    pub fn new(
        commands: Sender<AdminCommand>,
        status: Arc<Mutex<TvStatus>>,
        config_path: PathBuf,
    ) -> Self {
        Self {
            commands,
            status,
            config_path,
            writing: Arc::default(),
        }
    }

    fn status(&self) -> TvStatus {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct TvBody {
    /// Whether this server can control a TV.
    pub(super) controllable: bool,
    pub(super) channel: Option<u32>,
    /// `heavy`, `light` or `off`; `null` without a TV.
    pub(super) crt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TuneBody {
    channel: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct CrtBody {
    level: String,
}

pub(super) fn crt_name(crt: Crt) -> &'static str {
    match crt {
        Crt::Heavy => "heavy",
        Crt::Light => "light",
        Crt::Off => "off",
    }
}

/// `GET /tv`: what the TV is doing, or that there is none to control.
pub(super) async fn tv(State(state): State<AppState>) -> Json<TvBody> {
    Json(match &state.control {
        Some(control) => {
            let status = control.status();
            TvBody {
                controllable: true,
                channel: status.channel,
                crt: Some(crt_name(status.crt).to_string()),
            }
        }
        None => TvBody {
            controllable: false,
            channel: None,
            crt: None,
        },
    })
}

/// `POST /tune` with `{"channel": 5}`: asks the TV to change channel.
pub(super) async fn tune(
    State(state): State<AppState>,
    body: Result<Json<TuneBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(body) = body.map_err(|r| ApiError::new(r.status(), r.body_text()))?;
    if !state
        .config()
        .channels
        .iter()
        .any(|c| c.number == body.channel)
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no channel {}", body.channel),
        ));
    }
    send(&state, AdminCommand::Tune(body.channel))
}

/// `POST /crt` with `{"level": "light"}`: asks the TV to use a CRT filter.
pub(super) async fn crt(
    State(state): State<AppState>,
    body: Result<Json<CrtBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(body) = body.map_err(|r| ApiError::new(r.status(), r.body_text()))?;
    let level = Crt::parse(&body.level).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "`level` must be heavy, light or off",
        )
    })?;
    send(&state, AdminCommand::SetCrt(level))
}

/// Hands `command` to the TV. `202`: the run loop carries it out at its next
/// step, within a few tens of milliseconds.
pub(super) fn send(state: &AppState, command: AdminCommand) -> Result<StatusCode, ApiError> {
    let control = state.control.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "this server has no TV to control; the admin inside surfcult run does",
        )
    })?;
    control
        .commands
        .send(command)
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "the TV has stopped"))?;
    Ok(StatusCode::ACCEPTED)
}
