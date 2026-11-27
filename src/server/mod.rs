//! The read API: channels, the schedule for a time range and health, as JSON
//! over HTTP.
//!
//! [`router`] builds the Axum router and [`serve`] runs it on a listener the
//! caller binds to the address in [`ServerSettings`], which is
//! `127.0.0.1:8090` unless `SURFCULT_API_ADDR` says otherwise. When a token is configured every request must carry
//! `Authorization: Bearer <token>` or it gets `401`.
//!
//! Every route is a fixed `GET`. Nothing here runs a command, writes to the
//! store or reads a file: channels come from the parsed [`Config`] and the
//! rest from the [`Store`]. Any other method gets `405` and any other path
//! `404`.
//!
//! The server owns its own [`Store`], opened with [`Store::open_read_only`] on
//! the same database file as the composition root's. That open neither
//! creates the file nor migrates it, so the server never writes to the
//! database; `/health` answers `503` while the schema is not the one this
//! build expects. The file is in WAL mode, so this read connection does
//! not block the writer and sees each committed write. `Store` is `Send` but
//! not `Sync`, so it sits behind a `Mutex`, and each request reads it inside
//! `spawn_blocking` to keep SQLite off the async threads. Sharing the
//! composition root's `Store` instead would make the tuner wait on HTTP
//! requests for the lock.
//!
//! `/xmltv.xml` exports the guide as XMLTV.
//!
//! Endpoints and JSON shapes are in `docs/reference/api.md`.

mod settings;
#[cfg(test)]
mod tests;
mod xmltv;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Mode, Plan};
use crate::store::{Store, StoreError};
use crate::timeline::{Event, EventKind};

pub use settings::{
    ADDR_ENV, ApiToken, DEFAULT_ADDR, ServerSettings, SettingsError, TOKEN_ENV, TOKEN_FILE_ENV,
};

/// The schedule window when a request gives no `to`: a day from `from`.
pub const DEFAULT_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// The longest window one schedule request may ask for: the seven-day
/// horizon the schedule is kept filled to.
pub const MAX_WINDOW_MS: i64 = crate::schedule::HORIZON_MS;

/// The current time in Unix epoch milliseconds.
pub type Clock = fn() -> i64;

/// The system clock as a [`Clock`].
pub fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    store: Arc<Mutex<Store>>,
    token: Option<Arc<ApiToken>>,
    now: Clock,
}

/// The read API over `config` and `store`. With `token`, every request needs
/// `Authorization: Bearer <token>`. `now` answers "now" for the default
/// schedule window and for health.
pub fn router(config: Config, store: Store, token: Option<ApiToken>, now: Clock) -> Router {
    let state = AppState {
        config: Arc::new(config),
        store: Arc::new(Mutex::new(store)),
        token: token.map(Arc::new),
        now,
    };
    Router::new()
        .route("/channels", get(channels))
        .route("/schedule", get(schedule))
        .route("/xmltv.xml", get(xmltv::guide))
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

/// Serves the read API on `listener` until it fails. The caller binds the
/// listener, so it can report the bound address only once binding worked.
pub async fn serve(
    listener: tokio::net::TcpListener,
    token: Option<ApiToken>,
    config: Config,
    store: Store,
) -> std::io::Result<()> {
    axum::serve(listener, router(config, store, token, system_now_ms)).await
}

/// Lets a request through only if no token is configured or it carries
/// exactly `Authorization: Bearer <token>`.
async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(token) = &state.token else {
        return next.run(request).await;
    };
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if presented.is_some_and(|presented| token.matches(presented)) {
        return next.run(request).await;
    }
    let mut response =
        ApiError::new(StatusCode::UNAUTHORIZED, "a valid bearer token is required").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ChannelsBody {
    channels: Vec<ChannelBody>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ChannelBody {
    number: u32,
    name: String,
    /// `sequential`, `shuffle`, `weighted` or `blocks`.
    mode: String,
    grid_ms: Option<u64>,
}

async fn channels(State(state): State<AppState>) -> Json<ChannelsBody> {
    let channels = state
        .config
        .channels
        .iter()
        .map(|channel| ChannelBody {
            number: channel.number,
            name: channel.name.clone(),
            mode: match &channel.plan {
                Plan::Fixed { mode, .. } => mode_name(*mode),
                Plan::Blocks { .. } => "blocks",
            }
            .to_string(),
            grid_ms: channel
                .grid
                .map(|grid| u64::try_from(grid.as_millis()).unwrap_or(u64::MAX)),
        })
        .collect();
    Json(ChannelsBody { channels })
}

#[derive(Debug, Deserialize)]
struct ScheduleQuery {
    channel: Option<u32>,
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ScheduleBody {
    from_ms: i64,
    to_ms: i64,
    channels: Vec<ChannelSchedule>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ChannelSchedule {
    number: u32,
    events: Vec<EventBody>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct EventBody {
    start_ms: i64,
    end_ms: i64,
    media_id: String,
    media_offset_ms: u64,
    media_duration_ms: u64,
    /// `program`, `break`, `filler` or `ident`.
    kind: String,
}

impl From<Event> for EventBody {
    fn from(event: Event) -> Self {
        Self {
            start_ms: event.start_ms,
            end_ms: event.end_ms,
            media_id: event.media_id.as_str().to_string(),
            media_offset_ms: event.media_offset_ms,
            media_duration_ms: event.media_duration_ms,
            kind: kind_name(event.kind).to_string(),
        }
    }
}

/// Events overlapping `[from, to)` for one channel, or for every configured
/// channel when `channel` is left out. `from` defaults to now and `to` to a
/// day after `from`; the window may be at most seven days.
async fn schedule(
    State(state): State<AppState>,
    query: Result<Query<ScheduleQuery>, QueryRejection>,
) -> Result<Json<ScheduleBody>, ApiError> {
    let Query(query) =
        query.map_err(|rejection| ApiError::new(StatusCode::BAD_REQUEST, rejection.body_text()))?;
    let from_ms = query.from.unwrap_or_else(state.now);
    let to_ms = query
        .to
        .unwrap_or_else(|| from_ms.saturating_add(DEFAULT_WINDOW_MS));
    if to_ms <= from_ms {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "`to` must be after `from`",
        ));
    }
    // Checked: `from` and `to` far apart in i64 would overflow.
    if to_ms
        .checked_sub(from_ms)
        .is_none_or(|window| window > MAX_WINDOW_MS)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("the window may be at most {MAX_WINDOW_MS} ms (seven days)"),
        ));
    }
    let numbers: Vec<u32> = match query.channel {
        Some(number) if state.config.channels.iter().any(|c| c.number == number) => vec![number],
        Some(number) => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no channel {number}"),
            ));
        }
        None => state.config.channels.iter().map(|c| c.number).collect(),
    };
    let channels = read_store(&state, move |store| {
        numbers
            .into_iter()
            .map(|number| {
                let events = store.events_between(number, from_ms, to_ms)?;
                Ok(ChannelSchedule {
                    number,
                    events: events.into_iter().map(EventBody::from).collect(),
                })
            })
            .collect()
    })
    .await
    .map_err(|message| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message))?;
    Ok(Json(ScheduleBody {
        from_ms,
        to_ms,
        channels,
    }))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct HealthBody {
    status: String,
    now_ms: i64,
    schema_version: u32,
    library_synced_at_ms: Option<i64>,
    channels: Vec<ChannelHealth>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ChannelHealth {
    number: u32,
    /// When the channel's last stored event ends; `None` if it has none.
    scheduled_until_ms: Option<i64>,
}

/// `200` with what the store holds, or `503` if the store cannot be read or
/// its schema is not the version this build expects.
async fn health(State(state): State<AppState>) -> Result<Json<HealthBody>, ApiError> {
    let now_ms = (state.now)();
    let numbers: Vec<u32> = state.config.channels.iter().map(|c| c.number).collect();
    read_store(&state, move |store| {
        let schema_version = store.schema_version()?;
        let supported = Store::supported_schema_version();
        if schema_version != supported {
            return Ok(Err(format!(
                "the database schema is version {schema_version}, this build expects {supported}"
            )));
        }
        let channels = numbers
            .into_iter()
            .map(|number| {
                Ok(ChannelHealth {
                    number,
                    scheduled_until_ms: store.schedule_end_ms(number)?,
                })
            })
            .collect::<Result<_, StoreError>>()?;
        Ok(Ok(HealthBody {
            status: "ok".to_string(),
            now_ms,
            schema_version,
            library_synced_at_ms: store.library_synced_at_ms()?,
            channels,
        }))
    })
    .await
    .and_then(|body| body)
    .map(Json)
    .map_err(|message| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message))
}

/// Runs `read` against the store on a blocking thread. The error is the text
/// to send back.
async fn read_store<T: Send + 'static>(
    state: &AppState,
    read: impl FnOnce(&Store) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, String> {
    let store = Arc::clone(&state.store);
    tokio::task::spawn_blocking(move || {
        // Reads cannot leave the store half-changed, so a panic in an
        // earlier request does not make it unsafe to use.
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        read(&store).map_err(|error| error.to_string())
    })
    .await
    .unwrap_or_else(|_| Err("the store read did not finish".to_string()))
}

/// An error answer: a status and `{"error": "..."}`.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, Json(body)).into_response()
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Sequential => "sequential",
        Mode::Shuffle => "shuffle",
        Mode::Weighted => "weighted",
    }
}

fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Program => "program",
        EventKind::Break => "break",
        EventKind::Filler => "filler",
        EventKind::Ident => "ident",
    }
}
