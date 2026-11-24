use std::future::Future;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use serde::de::DeserializeOwned;
use tower::ServiceExt;

use super::*;
use crate::library::{MediaId, MediaItem, MediaKind};
use crate::store::LibraryEntry;

const EXAMPLE: &str = include_str!("../../config/channels.example.toml");
const NOW: i64 = 1_000_000;
const TOKEN: &str = "s3cret-token";

fn fixed_now() -> i64 {
    NOW
}

fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}

fn event(start_ms: i64, end_ms: i64, media: &str, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: 0,
        media_duration_ms: 60_000,
        kind,
    }
}

/// The example config; channel 3 has three events around `NOW`, channel 4
/// one, channel 5 none, and the library was synced at 42.
fn store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write_events(
            3,
            &[
                event(NOW - 60_000, NOW + 1_000, "a", EventKind::Program),
                event(NOW + 1_000, NOW + 2_000, "ad", EventKind::Break),
                event(NOW + 2_000, NOW + 62_000, "b", EventKind::Program),
            ],
        )
        .unwrap();
    store
        .write_events(4, &[event(NOW, NOW + 60_000, "m", EventKind::Program)])
        .unwrap();
    let entry = LibraryEntry {
        item: MediaItem::new("a", "A", MediaKind::Episode, 60_000),
        path: None,
    };
    store.replace_library(&[entry], &[], 42).unwrap();
    store
}

fn app(token: Option<&str>) -> Router {
    let config = Config::parse(EXAMPLE).unwrap();
    router(
        config,
        store(),
        token.map(|token| ApiToken::new(token).unwrap()),
        fixed_now,
    )
}

fn get_request(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn send(app: Router, request: Request<Body>) -> Response {
    run(app.oneshot(request)).unwrap()
}

fn json<T: DeserializeOwned>(response: Response) -> T {
    let bytes = run(to_bytes(response.into_body(), usize::MAX)).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn error_text(response: Response) -> String {
    let value: serde_json::Value = json(response);
    value["error"].as_str().unwrap().to_string()
}

#[test]
fn channels_come_from_the_config() {
    let response = send(app(None), get_request("/channels"));

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body: ChannelsBody = json(response);
    let summary: Vec<_> = body
        .channels
        .iter()
        .map(|c| (c.number, c.name.as_str(), c.mode.as_str(), c.grid_ms))
        .collect();
    assert_eq!(
        summary,
        vec![
            (3, "SATURDAY CARTOONS", "shuffle", Some(1_800_000)),
            (4, "MOVIE NIGHT", "sequential", None),
            (5, "ALL DAY CARTOONS", "blocks", None),
        ]
    );
}

#[test]
fn the_schedule_of_one_channel_covers_the_requested_range() {
    let uri = format!("/schedule?channel=3&from={NOW}&to={}", NOW + 1_500);
    let response = send(app(None), get_request(&uri));

    assert_eq!(response.status(), StatusCode::OK);
    let body: ScheduleBody = json(response);
    assert_eq!((body.from_ms, body.to_ms), (NOW, NOW + 1_500));
    assert_eq!(body.channels.len(), 1);
    assert_eq!(body.channels[0].number, 3);
    assert_eq!(
        body.channels[0].events,
        vec![
            EventBody {
                start_ms: NOW - 60_000,
                end_ms: NOW + 1_000,
                media_id: "a".into(),
                media_offset_ms: 0,
                media_duration_ms: 60_000,
                kind: "program".into(),
            },
            EventBody {
                start_ms: NOW + 1_000,
                end_ms: NOW + 2_000,
                media_id: "ad".into(),
                media_offset_ms: 0,
                media_duration_ms: 60_000,
                kind: "break".into(),
            },
        ]
    );
}

#[test]
fn without_parameters_the_schedule_is_every_channel_for_a_day_from_now() {
    let body: ScheduleBody = json(send(app(None), get_request("/schedule")));

    assert_eq!((body.from_ms, body.to_ms), (NOW, NOW + DEFAULT_WINDOW_MS));
    let counts: Vec<_> = body
        .channels
        .iter()
        .map(|c| (c.number, c.events.len()))
        .collect();
    assert_eq!(counts, vec![(3, 3), (4, 1), (5, 0)]);
}

#[test]
fn a_bad_schedule_request_is_refused() {
    let too_long = format!("/schedule?from=0&to={}", MAX_WINDOW_MS + 1);
    for uri in [
        "/schedule?from=10&to=10",
        "/schedule?from=10&to=5",
        too_long.as_str(),
        "/schedule?from=soon",
        "/schedule?channel=-1",
        "/schedule?from=-9000000000000000000&to=9000000000000000000",
        "/schedule?from=-9223372036854775808&to=9223372036854775807",
    ] {
        let response = send(app(None), get_request(uri));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert!(!error_text(response).is_empty());
    }

    let exactly_max = format!("/schedule?from=0&to={MAX_WINDOW_MS}");
    let response = send(app(None), get_request(&exactly_max));
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn an_unknown_channel_is_not_found() {
    let response = send(app(None), get_request("/schedule?channel=99"));

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_text(response), "no channel 99");
}

#[test]
fn health_reports_the_schema_the_sync_time_and_each_horizon() {
    let response = send(app(None), get_request("/health"));

    assert_eq!(response.status(), StatusCode::OK);
    let body: HealthBody = json(response);
    assert_eq!(
        body,
        HealthBody {
            status: "ok".into(),
            now_ms: NOW,
            schema_version: Store::open_in_memory().unwrap().schema_version().unwrap(),
            library_synced_at_ms: Some(42),
            channels: vec![
                ChannelHealth {
                    number: 3,
                    scheduled_until_ms: Some(NOW + 62_000),
                },
                ChannelHealth {
                    number: 4,
                    scheduled_until_ms: Some(NOW + 60_000),
                },
                ChannelHealth {
                    number: 5,
                    scheduled_until_ms: None,
                },
            ],
        }
    );
}

#[test]
fn health_is_unavailable_while_the_schema_is_older_than_this_build() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("surfcult.db");
    drop(Store::open(&path).unwrap());
    let supported = Store::supported_schema_version();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "DELETE FROM schema_migrations WHERE version = ?1",
            [supported],
        )
        .unwrap();
    let config = Config::parse(EXAMPLE).unwrap();
    let app = router(
        config,
        Store::open_read_only(&path).unwrap(),
        None,
        fixed_now,
    );

    let response = send(app, get_request("/health"));

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_text(response),
        format!(
            "the database schema is version {}, this build expects {supported}",
            supported - 1
        )
    );
}

#[test]
fn with_a_token_requests_without_it_get_401() {
    for authorization in [
        None,
        Some("Bearer wrong-token!"),
        Some("Bearer s3cret-toke"),
        Some("Bearer s3cret-tokenX"),
        Some("bearer s3cret-token"),
        Some("Basic s3cret-token"),
        Some("s3cret-token"),
    ] {
        for path in ["/channels", "/schedule", "/health", "/nothing-here"] {
            let mut request = Request::get(path);
            if let Some(value) = authorization {
                request = request.header(header::AUTHORIZATION, value);
            }
            let response = send(app(Some(TOKEN)), request.body(Body::empty()).unwrap());
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{path} with {authorization:?}"
            );
            assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
    }
}

#[test]
fn with_a_token_requests_that_carry_it_get_through() {
    for path in ["/channels", "/schedule", "/health"] {
        let request = Request::get(path)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let response = send(app(Some(TOKEN)), request);
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[test]
fn without_a_token_no_header_is_needed() {
    let response = send(app(None), get_request("/health"));
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn only_the_three_reads_exist() {
    for path in [
        "/",
        "/channels/../../etc/passwd",
        "/config/channels.example.toml",
        "/static/x",
    ] {
        let response = send(app(None), get_request(path));
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        for path in ["/channels", "/schedule", "/health"] {
            let request = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::from("{}"))
                .unwrap();
            let response = send(app(None), request);
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}"
            );
        }
    }
}

#[test]
fn the_server_listens_on_localhost_by_default() {
    let settings = ServerSettings::resolve(|_| None).unwrap();

    assert_eq!(settings, ServerSettings::default());
    assert_eq!(settings.addr, DEFAULT_ADDR);
    assert!(settings.addr.ip().is_loopback());
    assert_eq!(settings.token, None);
}

#[test]
fn a_configured_address_overrides_the_default() {
    let settings =
        ServerSettings::resolve(|name| (name == ADDR_ENV).then(|| "0.0.0.0:9000".into())).unwrap();
    assert_eq!(settings.addr, "0.0.0.0:9000".parse().unwrap());

    let error = ServerSettings::resolve(|name| (name == ADDR_ENV).then(|| "everywhere".into()))
        .unwrap_err();
    assert_eq!(error, SettingsError::BadAddr("everywhere".into()));
}

#[test]
fn the_token_comes_from_the_environment_or_a_file() {
    let settings =
        ServerSettings::resolve(|name| (name == TOKEN_ENV).then(|| format!(" {TOKEN}\n"))).unwrap();
    assert_eq!(settings.token, Some(ApiToken::new(TOKEN).unwrap()));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, format!("{TOKEN}\n")).unwrap();
    let path = path.to_str().unwrap().to_string();
    let settings =
        ServerSettings::resolve(|name| (name == TOKEN_FILE_ENV).then(|| path.clone())).unwrap();
    assert_eq!(settings.token, Some(ApiToken::new(TOKEN).unwrap()));
}

#[test]
fn a_token_file_that_cannot_be_read_is_an_error_not_an_open_server() {
    let missing = "/nonexistent/surfcult-token".to_string();
    let error = ServerSettings::resolve(|name| (name == TOKEN_FILE_ENV).then(|| missing.clone()))
        .unwrap_err();
    assert!(matches!(error, SettingsError::TokenFile { .. }));
}

#[test]
fn bad_tokens_are_refused_and_never_shown() {
    assert_eq!(ApiToken::new("  ").unwrap_err(), SettingsError::EmptyToken);
    assert_eq!(ApiToken::new("a b").unwrap_err(), SettingsError::BadToken);
    assert_eq!(
        ApiToken::new("a\u{7}").unwrap_err(),
        SettingsError::BadToken
    );

    let token = ApiToken::new(TOKEN).unwrap();
    assert_eq!(format!("{token:?}"), "ApiToken(<redacted>)");
    let settings = ServerSettings {
        addr: DEFAULT_ADDR,
        token: Some(token),
    };
    assert!(!format!("{settings:?}").contains(TOKEN));
}

#[test]
fn the_token_comparison_needs_an_exact_match() {
    let token = ApiToken::new(TOKEN).unwrap();
    assert!(token.matches(TOKEN));
    assert!(!token.matches(""));
    assert!(!token.matches("s3cret-tokeN"));
    assert!(!token.matches("s3cret-token "));
}

#[test]
fn serve_answers_on_a_listener_the_caller_bound() {
    use std::io::{Read, Write};

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Config::parse(EXAMPLE).unwrap();
        tokio::spawn(serve(listener, None, config, store()));
        tokio::task::spawn_blocking(move || {
            let mut stream = std::net::TcpStream::connect(addr).unwrap();
            stream
                .write_all(b"GET /channels HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        })
        .await
        .unwrap()
    });

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("SATURDAY CARTOONS"), "{response}");
}
