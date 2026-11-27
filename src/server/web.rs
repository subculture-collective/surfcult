//! The web admin: one page in the Cable Grid design
//! (`docs/design/cable-grid.md#web-admin`), compiled into the binary, and
//! the `/guide` read it draws its guide from.
//!
//! The page, its script, its stylesheet, the fonts and the mark hold no
//! data, so they are served without the bearer token. The page asks for the
//! token when a read answers `401` and sends it with every read; `/guide`,
//! like the other reads, needs it.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState, read_store};
use crate::guide::{self, Window};
use crate::library::{MediaKind, MediaSource};
use crate::overlay::program_title;

/// The guide window when a request gives no `to`: three hours.
pub const DEFAULT_GUIDE_MS: i64 = 3 * 60 * 60 * 1000;
/// The longest guide window one request may ask for: a day.
pub const MAX_GUIDE_MS: i64 = 24 * 60 * 60 * 1000;

const INDEX: &str = include_str!("../../assets/web/index.html");
const SCRIPT: &str = include_str!("../../assets/web/admin.js");
const STYLE: &str = include_str!("../../assets/web/admin.css");
const MARK: &str = include_str!("../../assets/brand/mark.svg");
const FAVICON: &str = include_str!("../../assets/brand/favicon.svg");
const ARCHIVO: &[u8] = include_bytes!("../../assets/fonts/ArchivoNarrow-Bold.ttf");
const VCR: &[u8] = include_bytes!("../../assets/fonts/VCR_OSD_MONO_1.001.ttf");

/// The page and what it loads, none of it behind the token.
pub(super) fn pages() -> Router<AppState> {
    Router::new()
        .route("/", get(|| file("text/html; charset=utf-8", INDEX)))
        .route(
            "/admin.js",
            get(|| file("text/javascript; charset=utf-8", SCRIPT)),
        )
        .route("/admin.css", get(|| file("text/css; charset=utf-8", STYLE)))
        .route("/mark.svg", get(|| file("image/svg+xml", MARK)))
        .route("/favicon.svg", get(|| file("image/svg+xml", FAVICON)))
        .route(
            "/fonts/archivo-narrow-bold.ttf",
            get(|| file("font/ttf", ARCHIVO)),
        )
        .route("/fonts/vcr-osd-mono.ttf", get(|| file("font/ttf", VCR)))
}

async fn file(content_type: &'static str, body: impl AsRef<[u8]>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // A new build changes these files; the browser checks each time.
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body.as_ref().to_vec(),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct GuideQuery {
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct GuideBody {
    pub(super) from_ms: i64,
    pub(super) to_ms: i64,
    pub(super) now_ms: i64,
    /// The TV's time zone, an IANA name such as `America/Chicago`, so the
    /// page shows the times the TV shows.
    pub(super) timezone: String,
    pub(super) channels: Vec<GuideChannel>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct GuideChannel {
    pub(super) number: u32,
    pub(super) name: String,
    pub(super) cells: Vec<GuideCellBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct GuideCellBody {
    pub(super) start_ms: i64,
    pub(super) end_ms: i64,
    /// `Series: Episode` for an episode, or `UNKNOWN PROGRAM`.
    pub(super) title: String,
    pub(super) movie: bool,
    pub(super) genres: Vec<String>,
}

/// Every channel's programs overlapping `[from, to)`, with titles: the
/// guide's cells, so a program cut by breaks is one cell and breaks are left
/// out. `from` defaults to now and `to` to three hours after it; the window
/// may be at most a day.
pub(super) async fn guide(
    State(state): State<AppState>,
    query: Result<Query<GuideQuery>, QueryRejection>,
) -> Result<Json<GuideBody>, ApiError> {
    let Query(query) =
        query.map_err(|rejection| ApiError::new(StatusCode::BAD_REQUEST, rejection.body_text()))?;
    let now_ms = (state.now)();
    let from_ms = query.from.unwrap_or(now_ms);
    let to_ms = query
        .to
        .unwrap_or_else(|| from_ms.saturating_add(DEFAULT_GUIDE_MS));
    if to_ms
        .checked_sub(from_ms)
        .is_none_or(|window| window <= 0 || window > MAX_GUIDE_MS)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "`to` must be after `from`, and at most a day after it",
        ));
    }
    let channels: Vec<(u32, String)> = state
        .config
        .channels
        .iter()
        .map(|c| (c.number, c.name.clone()))
        .collect();
    let numbers: Vec<u32> = channels.iter().map(|(n, _)| *n).collect();
    let window = Window { from_ms, to_ms };
    let rows = read_store(&state, move |store| {
        let rows = guide::build(store, store, &numbers, window)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let cells = row
                    .cells
                    .iter()
                    .map(|cell| GuideCellBody {
                        start_ms: cell.start_ms,
                        end_ms: cell.end_ms,
                        title: program_title(&cell.media_id, store),
                        movie: store
                            .resolve(&cell.media_id)
                            .is_ok_and(|item| item.kind == MediaKind::Movie),
                        genres: cell.genres.clone(),
                    })
                    .collect::<Vec<_>>();
                (row.channel, cells)
            })
            .collect::<Vec<_>>())
    })
    .await
    .map_err(|message| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message))?;
    let channels = channels
        .into_iter()
        .map(|(number, name)| GuideChannel {
            number,
            name,
            cells: rows
                .iter()
                .find(|(n, _)| *n == number)
                .map(|(_, cells)| cells.clone())
                .unwrap_or_default(),
        })
        .collect();
    Ok(Json(GuideBody {
        from_ms,
        to_ms,
        now_ms,
        timezone: state.config.timezone.name().to_string(),
        channels,
    }))
}
