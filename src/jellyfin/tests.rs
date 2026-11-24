//! Tests run against recorded Jellyfin 12.4.0 responses (names, paths and ids
//! are fake) and a fake transport. Only the last group opens a socket, and only
//! to a listener on 127.0.0.1 that the test starts itself.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::library::{MediaId, MediaKind, MediaSource, SourceError};
use crate::store::Store;

const ITEMS: &str = include_str!("../../tests/fixtures/jellyfin/items-page.json");
const BOXSETS: &str = include_str!("../../tests/fixtures/jellyfin/boxsets-page.json");
const CHILDREN: &str =
    include_str!("../../tests/fixtures/jellyfin/boxset-children-by-parent-id.json");

/// A value that stands for the API key. Tests look for it everywhere it must not be.
const SENTINEL: &str = "SENTINEL-KEY-0123456789abcdef";

fn key() -> ApiKey {
    ApiKey::new(SENTINEL).unwrap()
}

type Handler = Box<dyn Fn(&str) -> Result<String, HttpError> + Send + Sync>;
type PostHandler = Box<dyn Fn(&str, &str) -> Result<String, HttpError> + Send + Sync>;

/// One request a [`FakeHttp`] received.
#[derive(Debug, Clone)]
struct Call {
    method: &'static str,
    path: String,
    authorization: String,
    body: Option<String>,
}

/// A transport that answers from a closure and records every request. GET
/// and DELETE go to `handler` with the path; POST goes to `post` with the
/// path and the body when it is set, and to `handler` otherwise.
struct FakeHttp {
    handler: Handler,
    post: Option<PostHandler>,
    calls: Mutex<Vec<Call>>,
}

impl FakeHttp {
    fn new(
        handler: impl Fn(&str) -> Result<String, HttpError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            post: None,
            calls: Mutex::new(Vec::new()),
        })
    }

    /// A transport whose POST answers come from `post`. GET and DELETE answer
    /// with an empty body.
    fn with_post(
        post: impl Fn(&str, &str) -> Result<String, HttpError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(|_| Ok(String::new())),
            post: Some(Box::new(post)),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, method: &'static str, path: &str, authorization: &str, body: Option<&str>) {
        self.calls.lock().unwrap().push(Call {
            method,
            path: path.to_string(),
            authorization: authorization.to_string(),
            body: body.map(str::to_string),
        });
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// Serves the recorded fixtures, at most `page_cap` items per page whatever
    /// the client asks for.
    fn fixtures(page_cap: usize) -> Arc<Self> {
        Self::new(move |path| serve_fixtures(path, page_cap, ITEMS))
    }

    /// Every request as `(path_and_query, authorization)`.
    fn requests(&self) -> Vec<(String, String)> {
        self.calls()
            .into_iter()
            .map(|call| (call.path, call.authorization))
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|(path, _)| path).collect()
    }
}

impl JellyfinHttp for Arc<FakeHttp> {
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError> {
        self.record("GET", path_and_query, authorization, None);
        (self.handler)(path_and_query)
    }

    fn post_json(
        &self,
        path_and_query: &str,
        json: &str,
        authorization: &str,
    ) -> Result<String, HttpError> {
        self.record("POST", path_and_query, authorization, Some(json));
        match &self.post {
            Some(post) => post(path_and_query, json),
            None => (self.handler)(path_and_query),
        }
    }

    fn delete(&self, path_and_query: &str, authorization: &str) -> Result<(), HttpError> {
        self.record("DELETE", path_and_query, authorization, None);
        (self.handler)(path_and_query).map(drop)
    }
}

fn param<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn page(all: &[Value], path: &str, cap: usize) -> String {
    let start: usize = param(path, "StartIndex").unwrap().parse().unwrap();
    let limit: usize = param(path, "Limit").unwrap().parse().unwrap();
    let end = (start + limit.min(cap)).min(all.len());
    let slice = all.get(start..end).unwrap_or(&[]);
    json!({ "Items": slice, "TotalRecordCount": all.len(), "StartIndex": start }).to_string()
}

fn serve_fixtures(path: &str, cap: usize, items: &str) -> Result<String, HttpError> {
    let list = |body: &str| -> Vec<Value> {
        serde_json::from_str::<Value>(body).unwrap()["Items"]
            .as_array()
            .unwrap()
            .clone()
    };
    if let Some(parent) = param(path, "ParentId") {
        let children: Value = serde_json::from_str(CHILDREN).unwrap();
        let all = children[parent]["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        return Ok(page(&all, path, cap));
    }
    if param(path, "IncludeItemTypes") == Some("BoxSet") {
        return Ok(page(&list(BOXSETS), path, cap));
    }
    Ok(page(&list(items), path, cap))
}

/// When the tests say a sync happened, in Unix epoch milliseconds.
const SYNCED_AT: i64 = 1_767_225_600_000;

fn client(http: &Arc<FakeHttp>) -> JellyfinClient {
    JellyfinClient::with_transport(key(), http.clone())
}

/// A client on `http` and an empty in-memory store.
fn client_and_store(http: &Arc<FakeHttp>) -> (JellyfinClient, Store) {
    (client(http), Store::open_in_memory().unwrap())
}

fn ids(items: &[crate::library::MediaItem]) -> Vec<&str> {
    items.iter().map(|item| item.id.as_str()).collect()
}

const MOVIE_1: &str = "1e0ff4a8-a553-6961-2793-0e218769b7b7";
const EPISODE_2: &str = "010f6f59-58ca-cbc6-0c2f-14f3bde31d24";
const MOVIE_3: &str = "3282307a-d805-60f8-3e46-a3022f90087e";
const MOVIE_4: &str = "a57a957c-1ae6-5318-4137-e200281aa5af";

// ---- sync and the cache -------------------------------------------------

#[test]
fn a_sync_fills_the_cache_with_runtime_chapters_streams_and_path() {
    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!(
        report,
        SyncReport {
            items: 4,
            collections: 2,
            skipped: 1
        }
    );
    let movie = store.resolve(&MediaId::new(MOVIE_1)).unwrap();
    assert_eq!(movie.title, "Fixture Movie 1");
    assert_eq!(movie.kind, MediaKind::Movie);
    assert_eq!(movie.runtime_ms, 5_823_840, "58238400000 ticks / 10000");
    assert_eq!(movie.chapters.len(), 6);
    assert_eq!(movie.chapters[0].start_ms, 0);
    assert_eq!(movie.chapters[1].start_ms, 328_745);
    assert_eq!(movie.chapters[1].name.as_deref(), Some("Chapter 2"));
    assert_eq!(movie.video.len(), 1);
    assert_eq!(movie.video[0].codec, "hevc");
    assert_eq!((movie.video[0].width, movie.video[0].height), (1920, 1080));
    assert_eq!(movie.video[0].bit_depth, Some(10));
    assert_eq!(movie.audio.len(), 1);
    assert_eq!(movie.audio[0].codec, "eac3");
    assert_eq!(movie.audio[0].channels, 6);
    assert_eq!(movie.audio[0].language.as_deref(), Some("eng"));

    let entries = store.library_items().unwrap();
    assert_eq!(
        entries[0].path.as_deref(),
        Some("/media/fixtures/movie-1.mkv")
    );
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT));
}

#[test]
fn episodes_and_extra_streams_are_read() {
    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let episode = store.resolve(&MediaId::new(EPISODE_2)).unwrap();
    assert_eq!(episode.kind, MediaKind::Episode);
    assert_eq!(episode.runtime_ms, 1_436_105);
    assert_eq!(episode.audio[0].language.as_deref(), Some("jpn"));

    let movie_4 = store.resolve(&MediaId::new(MOVIE_4)).unwrap();
    let audio: Vec<_> = movie_4.audio.iter().map(|a| a.codec.as_str()).collect();
    assert_eq!(audio, ["truehd", "ac3"], "audio streams keep their order");

    let no_chapters = store.resolve(&MediaId::new(MOVIE_3)).unwrap();
    assert!(no_chapters.chapters.is_empty());
}

#[test]
fn a_placeholder_episode_with_no_streams_is_skipped() {
    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let items = store.list_items().unwrap();

    assert_eq!(ids(&items), [MOVIE_1, EPISODE_2, MOVIE_3, MOVIE_4]);
    assert!(items.iter().all(|item| !item.title.contains("missing")));
}

#[test]
fn collections_list_their_members_in_jellyfins_order() {
    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let collections = store.list_collections().unwrap();

    assert_eq!(collections.len(), 2);
    assert_eq!(collections[0].name, "Fixture Collection A");
    assert_eq!(
        collections[0].members,
        [MediaId::new(MOVIE_1), MediaId::new(MOVIE_3)]
    );
    assert_eq!(collections[1].name, "Fixture Collection B");
    assert_eq!(collections[1].members, [MediaId::new(MOVIE_4)]);
}

#[test]
fn collection_children_that_are_not_cached_items_are_left_out() {
    let http = FakeHttp::new(|path| {
        if param(path, "ParentId").is_some() {
            return Ok(json!({
                "Items": [
                    { "Id": MOVIE_1, "Type": "Movie" },
                    { "Id": "series-id", "Type": "Series" },
                    // The same id without dashes and in capitals still matches.
                    { "Id": MOVIE_3.replace('-', "").to_uppercase(), "Type": "Movie" },
                    { "Id": MOVIE_1, "Type": "Movie" },
                ],
                "TotalRecordCount": 4
            })
            .to_string());
        }
        serve_fixtures(path, 500, ITEMS)
    });
    let (client, mut store) = client_and_store(&http);
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let members = &store.list_collections().unwrap()[0].members;

    assert_eq!(members, &[MediaId::new(MOVIE_1), MediaId::new(MOVIE_3)]);
}

#[test]
fn resolving_an_unknown_id_is_not_found() {
    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let err = store.resolve(&MediaId::new("nope")).unwrap_err();

    assert_eq!(err, SourceError::NotFound(MediaId::new("nope")));
}

#[test]
fn before_any_sync_the_cache_is_empty() {
    let store = Store::open_in_memory().unwrap();

    assert!(store.list_items().unwrap().is_empty());
    assert!(store.list_collections().unwrap().is_empty());
    assert_eq!(store.library_synced_at_ms().unwrap(), None);
}

#[test]
fn syncing_again_replaces_items_that_went_away() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.db");
    let mut store = Store::open(&db).unwrap();
    sync(&client(&FakeHttp::fixtures(500)), &mut store, SYNCED_AT).unwrap();
    drop(store);
    let only_movie_3 = {
        let all: Value = serde_json::from_str(ITEMS).unwrap();
        let movie_3 = all["Items"][2].clone();
        json!({ "Items": [movie_3], "TotalRecordCount": 1 }).to_string()
    };
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &only_movie_3));
    let mut store = Store::open(&db).unwrap();

    sync(&client(&http), &mut store, SYNCED_AT + 1).unwrap();

    assert_eq!(ids(&store.list_items().unwrap()), [MOVIE_3]);
    // Collection A listed movies 1 and 3; only 3 remains in the cache.
    assert_eq!(
        store.list_collections().unwrap()[0].members,
        [MediaId::new(MOVIE_3)]
    );
}

// ---- paging -------------------------------------------------------------

#[test]
fn items_are_fetched_page_by_page() {
    let http = FakeHttp::fixtures(2);
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!(report.items, 4);
    let item_requests: Vec<String> = http
        .paths()
        .into_iter()
        .filter(|path| path.contains("IncludeItemTypes=Movie,Episode&Fields="))
        .collect();
    let starts: Vec<_> = item_requests
        .iter()
        .map(|p| param(p, "StartIndex").unwrap().to_string())
        .collect();
    assert_eq!(starts, ["0", "2", "4"], "5 listed items at 2 per page");
    assert!(
        item_requests
            .iter()
            .all(|p| param(p, "Limit") == Some("500"))
    );
}

#[test]
fn a_list_that_ends_before_its_count_is_an_error() {
    let http = FakeHttp::new(|_| Ok(json!({ "Items": [], "TotalRecordCount": 10 }).to_string()));
    let (client, mut store) = client_and_store(&http);

    let err = sync(&client, &mut store, SYNCED_AT).unwrap_err();

    assert!(matches!(err, JellyfinError::BadResponse(_)), "{err}");
}

#[test]
fn a_response_without_a_count_stops_at_a_short_page() {
    let http = FakeHttp::new(|_| Ok(json!({ "Items": [] }).to_string()));
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!((report.items, report.collections), (0, 0));
    assert_eq!(
        http.paths().len(),
        2,
        "one request for items, one for collections"
    );
}

// ---- parsing ------------------------------------------------------------

#[test]
fn unknown_fields_and_missing_optional_fields_do_not_break_parsing() {
    let items = json!({
        "Items": [
            {
                "Id": "a", "Type": "Movie", "RunTimeTicks": 6_000_000_000i64,
                "BrandNewJellyfinField": { "nested": [1, 2, 3] },
                "MediaStreams": [
                    { "Type": "Video", "Codec": "h264", "Width": 1280, "Height": 720,
                      "SomethingNew": true },
                    { "Type": "Subtitle", "Codec": "srt" },
                    { "Type": "Audio", "Codec": "aac", "Channels": 2 }
                ],
                "MediaSources": [{ "Path": "/media/a.mkv" }]
            },
            {
                "Id": "b", "Type": "Episode", "RunTimeTicks": 6_000_000_000i64,
                "Name": null, "Path": "", "Chapters": null,
                "MediaStreams": [{ "Type": "Video" }]
            }
        ],
        "TotalRecordCount": 2
    })
    .to_string();
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &items));
    let (client, mut store) = client_and_store(&http);

    sync(&client, &mut store, SYNCED_AT).unwrap();

    let a = store.resolve(&MediaId::new("a")).unwrap();
    assert_eq!(a.title, "a", "no Name falls back to the id");
    assert_eq!(a.runtime_ms, 600_000);
    assert_eq!(a.video[0].bit_depth, None);
    assert_eq!(a.audio[0].language, None);
    assert!(a.chapters.is_empty());
    let entries = store.library_items().unwrap();
    assert_eq!(
        entries[0].path.as_deref(),
        Some("/media/a.mkv"),
        "falls back to MediaSources"
    );
    let b = store.resolve(&MediaId::new("b")).unwrap();
    assert_eq!(b.video[0].codec, "unknown");
    assert_eq!(entries[1].path, None, "an empty Path is no path");
}

#[test]
fn items_that_are_not_playable_or_not_item_shaped_are_skipped_not_fatal() {
    let items = json!({
        "Items": [
            // A video stream but no runtime.
            { "Id": "no-runtime", "Type": "Movie",
              "MediaStreams": [{ "Type": "Video", "Codec": "h264", "Width": 1, "Height": 1 }] },
            // Audio only.
            { "Id": "audio-only", "Type": "Movie", "RunTimeTicks": 10_000_000i64,
              "MediaStreams": [{ "Type": "Audio", "Codec": "aac", "Channels": 2 }] },
            // Wrong type for a known field.
            { "Id": "bad-shape", "Type": "Movie", "RunTimeTicks": "long" },
            // Not a movie or episode.
            { "Id": "series", "Type": "Series", "RunTimeTicks": 10_000_000i64 },
            // Not an object at all.
            "text",
            { "Id": "good", "Type": "Movie", "RunTimeTicks": 10_000_000i64,
              "MediaStreams": [{ "Type": "Video", "Codec": "h264", "Width": 1, "Height": 1 }] }
        ],
        "TotalRecordCount": 6
    })
    .to_string();
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &items));
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!(report.items, 1);
    assert_eq!(report.skipped, 5);
    assert_eq!(ids(&store.list_items().unwrap()), ["good"]);
}

#[test]
fn an_item_listed_twice_is_cached_once() {
    let all: Value = serde_json::from_str(ITEMS).unwrap();
    let doubled = json!({
        "Items": [all["Items"][0].clone(), all["Items"][0].clone()],
        "TotalRecordCount": 2
    })
    .to_string();
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &doubled));
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!(report.items, 1);
}

// ---- failure leaves the cache alone ------------------------------------

fn snapshot(
    store: &Store,
) -> (
    Vec<crate::library::MediaItem>,
    Vec<crate::library::Collection>,
) {
    (
        store.list_items().unwrap(),
        store.list_collections().unwrap(),
    )
}

/// Syncs a good library, swaps nothing, then runs `failing` and checks that the
/// sync fails with an error matching `expected` and the cache is unchanged.
fn assert_failed_sync_keeps_cache(
    failing: impl Fn(&str) -> Result<String, HttpError> + Send + Sync + 'static,
    expected: impl Fn(&JellyfinError) -> bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.db");
    let mut store = Store::open(&db).unwrap();
    sync(&client(&FakeHttp::fixtures(500)), &mut store, SYNCED_AT).unwrap();
    let before = snapshot(&store);
    drop(store);

    let mut store = Store::open(&db).unwrap();
    let err = sync(&client(&FakeHttp::new(failing)), &mut store, SYNCED_AT + 1).unwrap_err();

    assert!(expected(&err), "unexpected error: {err}");
    assert_eq!(snapshot(&store), before);
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT));
}

#[test]
fn a_server_that_is_down_leaves_the_cache_untouched_and_readable() {
    assert_failed_sync_keeps_cache(
        |_| Err(HttpError::Unreachable("connection refused".into())),
        |err| matches!(err, JellyfinError::Unreachable(reason) if reason.contains("refused")),
    );
}

#[test]
fn a_401_is_an_unauthorized_error_and_leaves_the_cache_untouched() {
    assert_failed_sync_keeps_cache(
        |_| Err(HttpError::Status(401)),
        |err| *err == JellyfinError::Unauthorized,
    );
}

#[test]
fn another_error_status_is_reported_with_its_code() {
    assert_failed_sync_keeps_cache(
        |_| Err(HttpError::Status(503)),
        |err| *err == JellyfinError::Status(503),
    );
}

#[test]
fn a_malformed_body_is_an_error_and_leaves_the_cache_untouched() {
    assert_failed_sync_keeps_cache(
        |_| Ok("<html>not json</html>".to_string()),
        |err| matches!(err, JellyfinError::BadResponse(_)),
    );
}

#[test]
fn a_body_without_an_items_list_is_a_bad_response() {
    assert_failed_sync_keeps_cache(
        |_| Ok(json!({ "Message": "hello" }).to_string()),
        |err| matches!(err, JellyfinError::BadResponse(_)),
    );
}

#[test]
fn a_failure_after_the_items_were_fetched_still_leaves_the_cache_untouched() {
    // The items page is fine; listing collections fails.
    assert_failed_sync_keeps_cache(
        |path| {
            if param(path, "IncludeItemTypes") == Some("BoxSet") {
                Err(HttpError::Status(500))
            } else {
                serve_fixtures(path, 500, ITEMS)
            }
        },
        |err| *err == JellyfinError::Status(500),
    );
}

#[test]
fn a_failure_while_listing_one_collection_still_leaves_the_cache_untouched() {
    assert_failed_sync_keeps_cache(
        |path| {
            if param(path, "ParentId").is_some() {
                Err(HttpError::Unreachable("reset".into()))
            } else {
                serve_fixtures(path, 500, ITEMS)
            }
        },
        |err| matches!(err, JellyfinError::Unreachable(_)),
    );
}

#[test]
fn cached_items_are_available_to_a_new_process_while_jellyfin_is_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.db");
    let mut store = Store::open(&db).unwrap();
    sync(&client(&FakeHttp::fixtures(500)), &mut store, SYNCED_AT).unwrap();
    drop(store);

    // A later start: the server is down, and nothing is asked of it for reads.
    let http = FakeHttp::new(|_| Err(HttpError::Unreachable("no route to host".into())));
    let mut store = Store::open(&db).unwrap();

    assert_eq!(store.list_items().unwrap().len(), 4);
    assert_eq!(
        store.resolve(&MediaId::new(MOVIE_1)).unwrap().runtime_ms,
        5_823_840
    );
    assert_eq!(store.list_collections().unwrap().len(), 2);
    assert!(http.requests().is_empty(), "reads never go to Jellyfin");
    assert!(sync(&client(&http), &mut store, SYNCED_AT + 1).is_err());
}

// ---- the API key --------------------------------------------------------

#[test]
fn the_key_is_sent_only_in_the_authorization_header() {
    let http = FakeHttp::fixtures(2);
    let (client, mut store) = client_and_store(&http);

    sync(&client, &mut store, SYNCED_AT).unwrap();

    let requests = http.requests();
    assert!(requests.len() > 5);
    let expected = format!("MediaBrowser Token=\"{SENTINEL}\"");
    for (path, authorization) in &requests {
        assert_eq!(authorization, &expected);
        assert!(!path.contains(SENTINEL), "key in URL {path}");
        assert!(
            !path.to_lowercase().contains("api_key"),
            "api_key in {path}"
        );
    }
}

#[test]
fn the_key_is_not_in_debug_output_or_error_text() {
    let http = FakeHttp::fixtures(500);
    let client = JellyfinClient::with_transport(key(), http.clone());
    let config = JellyfinConfig {
        base_url: "http://jellyfin.example:8096".into(),
        key: key(),
    };
    let mut store = Store::open_in_memory().unwrap();
    sync(&client, &mut store, SYNCED_AT).unwrap();

    // `client` names the client above, so the helper is reached by another name.
    let with_transport = self::client;
    let mut texts = vec![
        format!("{:?}", key()),
        format!("{client:?}"),
        format!("{config:?}"),
        format!("{:#?}", config),
        format!("{:?}", UreqHttp::new(&config.base_url)),
    ];
    // Every error the client can produce, including a transport that repeats
    // the header it was sent in its own error text.
    let leaky = FakeHttp::new(|_| {
        Err(HttpError::Unreachable(format!(
            "failed with Authorization: MediaBrowser Token=\"{SENTINEL}\""
        )))
    });
    let err = sync(&with_transport(&leaky), &mut store, SYNCED_AT).unwrap_err();
    assert!(matches!(err, JellyfinError::Unreachable(_)));
    assert!(err.to_string().contains("[redacted]"), "{err}");
    texts.push(err.to_string());
    texts.push(format!("{err:?}"));
    for failure in [
        JellyfinError::Unauthorized,
        JellyfinError::Status(500),
        JellyfinError::BadResponse("bad".into()),
    ] {
        texts.push(failure.to_string());
    }
    let unauthorized = FakeHttp::new(|_| Err(HttpError::Status(401)));
    texts.push(
        sync(&with_transport(&unauthorized), &mut store, SYNCED_AT)
            .unwrap_err()
            .to_string(),
    );
    let malformed = FakeHttp::new(|_| Ok(format!("not json {SENTINEL}")));
    let err = sync(&with_transport(&malformed), &mut store, SYNCED_AT).unwrap_err();
    texts.push(err.to_string());
    let source_err = SourceError::Unavailable(err.to_string());
    texts.push(source_err.to_string());

    for text in &texts {
        assert!(!text.contains(SENTINEL), "key leaked in: {text}");
    }
}

#[test]
fn the_key_is_read_from_the_environment_first() {
    let key = ApiKey::resolve(|name| match name {
        KEY_ENV => Some(format!("  {SENTINEL}\n")),
        KEY_FILE_ENV => Some("/does/not/exist".into()),
        _ => None,
    })
    .unwrap();

    assert_eq!(key, ApiKey::new(SENTINEL).unwrap());
}

#[test]
fn the_key_is_read_from_a_secret_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("jellyfin-key");
    std::fs::write(&file, format!("{SENTINEL}\n")).unwrap();
    let path = file.to_str().unwrap().to_string();

    let key = ApiKey::resolve(|name| (name == KEY_FILE_ENV).then(|| path.clone())).unwrap();

    assert_eq!(key, ApiKey::new(SENTINEL).unwrap());
}

#[test]
fn an_empty_environment_variable_counts_as_unset() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("jellyfin-key");
    std::fs::write(&file, SENTINEL).unwrap();
    let path = file.to_str().unwrap().to_string();

    let key = ApiKey::resolve(|name| match name {
        KEY_ENV => Some("  ".into()),
        _ => Some(path.clone()),
    })
    .unwrap();

    assert_eq!(key, ApiKey::new(SENTINEL).unwrap());
}

#[test]
fn a_missing_empty_or_unreadable_key_is_a_clear_error_without_the_key() {
    assert_eq!(
        ApiKey::resolve(|_| None).unwrap_err(),
        KeyError::NotConfigured
    );
    assert_eq!(ApiKey::new("   ").unwrap_err(), KeyError::Empty);

    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    std::fs::write(&empty, "\n").unwrap();
    let empty_path = empty.to_str().unwrap().to_string();
    assert_eq!(
        ApiKey::resolve(|n| (n == KEY_FILE_ENV).then(|| empty_path.clone())).unwrap_err(),
        KeyError::Empty
    );

    let missing = dir.path().join("missing").to_str().unwrap().to_string();
    let err = ApiKey::resolve(|n| (n == KEY_FILE_ENV).then(|| missing.clone())).unwrap_err();
    assert!(matches!(err, KeyError::File { .. }));
    assert!(err.to_string().contains(&missing));
}

#[test]
fn a_key_that_would_break_the_header_is_refused_without_echoing_it() {
    for bad in [
        format!("{SENTINEL}\"quote"),
        format!("{SENTINEL}\\slash"),
        format!("{SENTINEL}\nsecond line"),
    ] {
        let err = ApiKey::new(&bad).unwrap_err();
        assert_eq!(err, KeyError::BadCharacters);
        assert!(!err.to_string().contains(SENTINEL));
        assert!(!format!("{err:?}").contains(SENTINEL));
    }
}

// ---- playback URLs --------------------------------------------------------

const DIRECT_INFO: &str = include_str!("../../tests/fixtures/jellyfin/playback-info-direct.json");
const TRANSCODE_INFO: &str =
    include_str!("../../tests/fixtures/jellyfin/playback-info-transcode.json");
const PLAY_SESSION: &str = "5f0d7e3c2b1a49e8a6c4d2b0f9e8d7c6";

/// The first video and audio stream of an item, as Jellyfin describes them.
struct Streams<'a> {
    video: &'a str,
    height: u32,
    depth: Option<u8>,
    range: &'a str,
    audio: &'a str,
}

/// Whether Jellyfin would direct-play `streams` under `profile`: both codecs
/// are in the direct-play entry and every condition of the video codec's codec
/// profile holds, where a condition on a value the stream lacks passes unless
/// it is required. This follows Jellyfin's rules only as far as surfcult's
/// profile uses them, and fails on anything else it sends.
fn jellyfin_direct_plays(profile: &Value, streams: &Streams) -> bool {
    let direct = &profile["DirectPlayProfiles"][0];
    assert!(direct.get("Container").is_none(), "any container plays");
    let listed = |field: &str, codec: &str| {
        direct[field]
            .as_str()
            .unwrap()
            .split(',')
            .any(|c| c == codec)
    };
    if !listed("VideoCodec", streams.video) || !listed("AudioCodec", streams.audio) {
        return false;
    }
    profile["CodecProfiles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|codec| codec["Type"] == "Video" && codec["Codec"] == streams.video)
        .flat_map(|codec| codec["Conditions"].as_array().unwrap())
        .all(|condition| {
            let expected = condition["Value"].as_str().unwrap();
            let actual = match condition["Property"].as_str().unwrap() {
                "VideoBitDepth" => streams.depth.map(|depth| depth.to_string()),
                "Height" => Some(streams.height.to_string()),
                "VideoRangeType" => Some(streams.range.to_string()),
                other => panic!("unexpected property {other}"),
            };
            let Some(actual) = actual else {
                return condition["IsRequired"] != true;
            };
            match condition["Condition"].as_str().unwrap() {
                "LessThanEqual" => {
                    actual.parse::<u64>().unwrap() <= expected.parse::<u64>().unwrap()
                }
                "EqualsAny" => expected.split('|').any(|value| value == actual),
                other => panic!("unexpected condition {other}"),
            }
        })
}

/// A Jellyfin stand-in for `POST /Items/{id}/PlaybackInfo`. It looks the item
/// up in the items fixture, applies the device profile the client sent with
/// [`jellyfin_direct_plays`], and answers with the recorded direct-play or
/// transcode response, rewritten to name the requested item.
fn fake_jellyfin() -> Arc<FakeHttp> {
    FakeHttp::with_post(|path, body| {
        let id = path
            .strip_prefix("/Items/")
            .and_then(|rest| rest.strip_suffix("/PlaybackInfo"))
            .unwrap_or_else(|| panic!("unexpected POST {path}"));
        let request: Value = serde_json::from_str(body).unwrap();
        let items: Value = serde_json::from_str(ITEMS).unwrap();
        let item = items["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["Id"] == id)
            .ok_or(HttpError::Status(404))?;
        let first = |kind: &str| {
            item["MediaStreams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stream| stream["Type"] == kind)
                .unwrap()
                .clone()
        };
        let (video, audio) = (first("Video"), first("Audio"));
        let streams = Streams {
            video: video["Codec"].as_str().unwrap(),
            height: video["Height"].as_u64().unwrap() as u32,
            depth: video["BitDepth"].as_u64().map(|depth| depth as u8),
            range: video["VideoRangeType"].as_str().unwrap(),
            audio: audio["Codec"].as_str().unwrap(),
        };
        let info = if jellyfin_direct_plays(&request["DeviceProfile"], &streams) {
            DIRECT_INFO
        } else {
            TRANSCODE_INFO
        };
        Ok(for_item(info, id))
    })
}

/// A playback-info fixture rewritten so its ids name `id` instead of the item
/// it was made for.
fn for_item(info: &str, id: &str) -> String {
    let undashed = |id: &str| id.replace('-', "");
    [MOVIE_1, MOVIE_3]
        .iter()
        .fold(info.to_string(), |info, fixture| {
            info.replace(fixture, id)
                .replace(&undashed(fixture), &undashed(id))
        })
}

/// The values of every query parameter called `name`, in any case.
fn params<'a>(path: &'a str, name: &str) -> Vec<&'a str> {
    let query = path.split_once('?').map_or("", |(_, query)| query);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
        .collect()
}

/// The one value of the query parameter `name`.
fn only(path: &str, name: &str) -> String {
    let values = params(path, name);
    assert_eq!(values.len(), 1, "{name} in {path}");
    values[0].to_string()
}

#[test]
fn an_h265_1080p_item_gets_a_direct_play_url() {
    let http = fake_jellyfin();

    let playback = client(&http)
        .playback(&MediaId::new(MOVIE_1), 600_000)
        .unwrap();

    assert_eq!(
        playback,
        Playback::Direct {
            path_and_query: format!(
                "/Videos/{MOVIE_1}/stream?static=true&mediaSourceId=1e0ff4a8a553696127930e218769b7b7"
            ),
            seek_ms: 600_000,
        }
    );
    assert_eq!(playback.transcode(), None);
    assert_eq!(
        playback.url("http://jellyfin.example:8096/"),
        format!("http://jellyfin.example:8096{}", playback.path_and_query())
    );
}

#[test]
fn an_item_outside_the_profile_gets_a_transcode_url() {
    let http = fake_jellyfin();
    let client = client(&http);

    // AV1, and H.264 at 10 bit.
    for id in [MOVIE_3, EPISODE_2] {
        let playback = client.playback(&MediaId::new(id), 0).unwrap();
        let Playback::Transcode {
            path_and_query,
            session,
        } = &playback
        else {
            panic!("{id} should be transcoded: {playback:?}");
        };
        assert!(
            path_and_query.starts_with(&format!("/videos/{id}/stream.ts?")),
            "{path_and_query}"
        );
        assert_eq!(
            session,
            &TranscodeSession {
                device_id: DEVICE_ID.into(),
                play_session_id: PLAY_SESSION.into(),
            }
        );
        assert_eq!(playback.transcode(), Some(session));
    }
}

#[test]
fn the_transcode_url_starts_at_the_start_position_and_follows_the_adr() {
    let http = fake_jellyfin();

    let playback = client(&http)
        .playback(&MediaId::new(MOVIE_3), 2_520_000)
        .unwrap();

    let path = playback.path_and_query();
    assert_eq!(
        only(path, "StartTimeTicks"),
        "25200000000",
        "42 min in ticks"
    );
    assert_eq!(only(path, "VideoCodec"), "h264");
    assert_eq!(
        only(path, "h264-videobitdepth"),
        "8",
        "a 10-bit H.264 source is re-encoded, never stream-copied"
    );
    assert_eq!(only(path, "AudioCodec"), "aac");
    assert_eq!(
        only(path, "VideoBitrate"),
        "8000000",
        "Jellyfin's 200 Mbps is replaced"
    );
    assert_eq!(only(path, "MaxWidth"), "1920");
    assert_eq!(only(path, "MaxHeight"), "1080");
    assert_eq!(only(path, "DeviceId"), DEVICE_ID, "Jellyfin's is replaced");
    assert_eq!(only(path, "PlaySessionId"), PLAY_SESSION);
    // What Jellyfin chose and surfcult does not override is kept.
    assert_eq!(
        only(path, "MediaSourceId"),
        "3282307ad80560f83e46a3022f90087e"
    );
    assert_eq!(only(path, "AudioStreamIndex"), "1");
    assert!(params(path, "api_key").is_empty(), "{path}");
}

#[test]
fn the_playback_request_carries_the_profile_and_start_position() {
    let http = fake_jellyfin();

    client(&http)
        .playback(&MediaId::new(MOVIE_1), 1_500)
        .unwrap();

    let calls = http.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[0].path, format!("/Items/{MOVIE_1}/PlaybackInfo"));
    let body: Value = serde_json::from_str(calls[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["DeviceProfile"], super::playback::device_profile());
    assert_eq!(body["StartTimeTicks"], 15_000_000);
    assert_eq!(body["SubtitleStreamIndex"], -1, "subtitles off");
    assert_eq!(body["EnableTranscoding"], true);
    let transcoding = &body["DeviceProfile"]["TranscodingProfiles"];
    assert_eq!(
        transcoding,
        &json!([{
            "Type": "Video",
            "Container": "ts",
            "VideoCodec": "h264",
            "AudioCodec": "aac",
            "Protocol": "http",
            "Context": "Streaming",
        }]),
        "a progressive MPEG-TS transcode, not HLS"
    );
}

#[test]
fn the_device_profile_matches_the_census_decision() {
    let profile = super::playback::device_profile();
    let video = |video, height, depth, range| Streams {
        video,
        height,
        depth,
        range,
        audio: "aac",
    };
    let audio = |audio| Streams {
        audio,
        ..video("hevc", 1080, Some(10), "SDR")
    };
    // ADR 0001, decision 3.
    let cases = [
        (
            "H.265 8-bit 1080p",
            video("hevc", 1080, Some(8), "SDR"),
            true,
        ),
        (
            "H.265 10-bit 1080p",
            video("hevc", 1080, Some(10), "SDR"),
            true,
        ),
        (
            "H.264 8-bit 1080p",
            video("h264", 1080, Some(8), "SDR"),
            true,
        ),
        (
            "H.264, bit depth unknown",
            video("h264", 720, None, "SDR"),
            true,
        ),
        ("H.264 10-bit", video("h264", 1080, Some(10), "SDR"), false),
        ("AV1", video("av1", 1080, Some(10), "SDR"), false),
        ("VVC", video("vvc", 1080, Some(10), "SDR"), false),
        ("VC-1", video("vc1", 1080, Some(8), "SDR"), false),
        ("MPEG-2", video("mpeg2video", 1080, Some(8), "SDR"), false),
        ("VP9", video("vp9", 1080, Some(8), "SDR"), false),
        ("MPEG-1", video("mpeg1video", 480, Some(8), "SDR"), false),
        (
            "H.265 above 1080p",
            video("hevc", 2160, Some(10), "SDR"),
            false,
        ),
        ("HDR10", video("hevc", 1080, Some(10), "HDR10"), false),
        (
            "Dolby Vision",
            video("hevc", 1080, Some(10), "DOVIWithHDR10"),
            false,
        ),
        ("HLG", video("hevc", 1080, Some(10), "HLG"), false),
        ("TrueHD", audio("truehd"), true),
        ("DTS", audio("dts"), true),
        ("E-AC3", audio("eac3"), true),
        ("PCM", audio("pcm_s16le"), false),
    ];
    for (name, streams, direct) in &cases {
        assert_eq!(jellyfin_direct_plays(&profile, streams), *direct, "{name}");
        // The tuner's `needs_transcode` reads the same rules; it cannot see HDR.
        if streams.range == "SDR" {
            let mut item = crate::library::MediaItem::new("x", "X", MediaKind::Movie, 1);
            item.video.push(crate::library::VideoStream {
                codec: streams.video.into(),
                width: 0,
                height: streams.height,
                bit_depth: streams.depth,
            });
            item.audio.push(crate::library::AudioStream {
                codec: streams.audio.into(),
                channels: 2,
                language: None,
            });
            assert_eq!(
                crate::library::profile::needs_transcode(&item),
                !direct,
                "{name}"
            );
        }
    }
}

#[test]
fn stopping_a_transcode_deletes_its_active_encoding() {
    let http = fake_jellyfin();
    let client = client(&http);
    let playback = client.playback(&MediaId::new(MOVIE_3), 0).unwrap();

    client
        .stop_transcode(playback.transcode().unwrap())
        .unwrap();

    let calls = http.calls();
    let stop = calls.last().unwrap();
    assert_eq!(stop.method, "DELETE");
    assert_eq!(
        stop.path,
        format!("/Videos/ActiveEncodings?DeviceId={DEVICE_ID}&PlaySessionId={PLAY_SESSION}")
    );
    // The transcode URL names the same device and session, so this is the
    // transcode the delete stops.
    assert_eq!(only(playback.path_and_query(), "DeviceId"), DEVICE_ID);
    assert_eq!(
        only(playback.path_and_query(), "PlaySessionId"),
        PLAY_SESSION
    );

    // Jellyfin answers 404 for a session it no longer has: already stopped.
    let ended = FakeHttp::new(|_| Err(HttpError::Status(404)));
    let session = playback.transcode().unwrap();
    assert_eq!(self::client(&ended).stop_transcode(session), Ok(()));

    let refused = FakeHttp::new(|_| Err(HttpError::Status(401)));
    assert_eq!(
        self::client(&refused).stop_transcode(session),
        Err(JellyfinError::Unauthorized)
    );
}

#[test]
fn nothing_is_reported_to_jellyfins_playback_sessions() {
    let http = fake_jellyfin();
    let client = client(&http);
    for id in [MOVIE_1, MOVIE_3, EPISODE_2, MOVIE_4] {
        let playback = client.playback(&MediaId::new(id), 60_000).unwrap();
        if let Some(session) = playback.transcode() {
            client.stop_transcode(session).unwrap();
        }
    }

    let calls = http.calls();
    assert!(calls.iter().any(|call| call.method == "DELETE"));
    for call in &calls {
        let path = call.path.to_lowercase();
        assert!(!path.contains("sessions/playing"), "{}", call.path);
        assert!(!path.contains("/sessions"), "{}", call.path);
        assert!(!path.contains("playeditems"), "{}", call.path);
    }
}

#[test]
fn the_key_never_appears_in_a_playback_url_request_or_debug_output() {
    let expected = format!("MediaBrowser Token=\"{SENTINEL}\"");
    for spelling in ["api_key=", "ApiKey=", "API_KEY="] {
        let info = TRANSCODE_INFO.replace("api_key=", spelling);
        let http = FakeHttp::with_post(move |_, _| Ok(info.clone()));
        let client = client(&http);

        let playback = client.playback(&MediaId::new(MOVIE_3), 0).unwrap();
        client
            .stop_transcode(playback.transcode().unwrap())
            .unwrap();

        for text in [
            playback.path_and_query().to_string(),
            playback.url("http://jellyfin.example:8096"),
            format!("{playback:?}"),
        ] {
            assert!(!text.contains(SENTINEL), "key leaked in: {text}");
            assert!(!text.to_lowercase().contains("api_key"), "{text}");
            assert!(!text.to_lowercase().contains("apikey"), "{text}");
        }
        for call in http.calls() {
            assert_eq!(call.authorization, expected);
            assert!(!call.path.contains(SENTINEL), "key in URL {}", call.path);
            assert!(!call.body.unwrap_or_default().contains(SENTINEL));
        }
    }
}

#[test]
fn a_refusal_or_an_answer_surfcult_cannot_use_is_an_error_without_the_key() {
    let answer = |body: String| {
        let http = FakeHttp::with_post(move |_, _| Ok(body.clone()));
        client(&http)
            .playback(&MediaId::new(MOVIE_3), 0)
            .unwrap_err()
    };

    let refused =
        answer(json!({ "MediaSources": [], "ErrorCode": "NoCompatibleStream" }).to_string());
    assert_eq!(
        refused,
        JellyfinError::NoPlayback("NoCompatibleStream".into())
    );
    assert!(matches!(
        answer(json!({ "MediaSources": [] }).to_string()),
        JellyfinError::NoPlayback(_)
    ));
    let neither = json!({
        "MediaSources": [{ "Id": "m", "SupportsDirectPlay": false, "SupportsTranscoding": false }],
        "PlaySessionId": PLAY_SESSION,
    });
    assert!(matches!(
        answer(neither.to_string()),
        JellyfinError::NoPlayback(_)
    ));

    // HLS is never used: it took about 19 s to start at an offset.
    let hls = TRANSCODE_INFO.replace("/stream.ts?", "/master.m3u8?");
    let hls = answer(hls);
    assert!(matches!(hls, JellyfinError::BadResponse(_)), "{hls:?}");
    let no_session = TRANSCODE_INFO.replace(
        &format!("\"PlaySessionId\": \"{PLAY_SESSION}\""),
        "\"PlaySessionId\": null",
    );
    assert!(matches!(answer(no_session), JellyfinError::BadResponse(_)));
    let malformed = answer(format!("not json {SENTINEL}"));
    assert!(matches!(malformed, JellyfinError::BadResponse(_)));

    let other_item = answer(TRANSCODE_INFO.replace(MOVIE_3, EPISODE_2));
    assert_eq!(
        other_item,
        JellyfinError::WrongItem(EPISODE_2.into()),
        "a transcode of another item is refused"
    );

    let failed = FakeHttp::with_post(|_, _| Err(HttpError::Status(404)));
    assert_eq!(
        client(&failed).playback(&MediaId::new("nope"), 0),
        Err(JellyfinError::Status(404))
    );

    for err in [refused, hls, malformed] {
        assert!(!err.to_string().contains(SENTINEL), "{err}");
        assert!(!format!("{err:?}").contains(SENTINEL), "{err:?}");
    }
}

// ---- the real transport, against a listener this test starts ------------

/// Accepts one connection, returns the raw request text (headers and any
/// `Content-Length` body) and answers with `reply`.
fn serve_once(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        let complete = |request: &[u8]| {
            let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                return false;
            };
            let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse().unwrap());
            request.len() >= end + 4 + length
        };
        while !complete(&request) {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
        stream.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (base, handle)
}

#[test]
fn the_real_transport_sends_the_header_and_returns_the_body() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\n\
         Connection: close\r\n\r\n{\"Items\":[\"ok\"]}\n",
    );
    let http = UreqHttp::new(&format!("{base}/"));

    let body = http
        .get("/Items?Recursive=true", &key().authorization_header())
        .unwrap();

    assert_eq!(body.trim(), "{\"Items\":[\"ok\"]}");
    let request = request.join().unwrap();
    let first_line = request.lines().next().unwrap();
    assert_eq!(first_line, "GET /Items?Recursive=true HTTP/1.1");
    assert!(
        request.to_lowercase().contains(&format!(
            "authorization: mediabrowser token=\"{}\"",
            SENTINEL.to_lowercase()
        )),
        "request was:\n{request}"
    );
    assert!(!first_line.contains(SENTINEL));
}

#[test]
fn the_real_transport_reports_401_and_does_not_follow_redirects() {
    let (base, _) =
        serve_once("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert_eq!(
        UreqHttp::new(&base).get("/Items", "x").unwrap_err(),
        HttpError::Status(401)
    );

    let (base, _) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        UreqHttp::new(&base).get("/Items", "x").unwrap_err(),
        HttpError::Status(302)
    );
}

#[test]
fn the_real_transport_reports_an_unreachable_server() {
    // Bind to get a free port, then close it so nothing listens there.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let err = UreqHttp::new(&format!("http://127.0.0.1:{port}"))
        .get("/Items", "x")
        .unwrap_err();

    assert!(matches!(err, HttpError::Unreachable(_)), "{err:?}");
}

#[test]
fn the_real_transport_posts_json_with_the_header() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\
         Connection: close\r\n\r\n{}",
    );

    let body = UreqHttp::new(&base)
        .post_json(
            "/Items/x/PlaybackInfo",
            "{\"StartTimeTicks\":0}",
            &key().authorization_header(),
        )
        .unwrap();

    assert_eq!(body, "{}");
    let request = request.join().unwrap();
    assert_eq!(
        request.lines().next().unwrap(),
        "POST /Items/x/PlaybackInfo HTTP/1.1"
    );
    let lower = request.to_lowercase();
    assert!(
        lower.contains("content-type: application/json"),
        "{request}"
    );
    assert!(
        lower.contains(&format!(
            "authorization: mediabrowser token=\"{}\"",
            SENTINEL.to_lowercase()
        )),
        "{request}"
    );
    assert!(request.ends_with("{\"StartTimeTicks\":0}"), "{request}");
}

#[test]
fn the_real_transport_sends_a_delete_and_reports_its_status() {
    let (base, request) =
        serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    UreqHttp::new(&base)
        .delete("/Videos/ActiveEncodings?DeviceId=d&PlaySessionId=p", "x")
        .unwrap();
    assert_eq!(
        request.join().unwrap().lines().next().unwrap(),
        "DELETE /Videos/ActiveEncodings?DeviceId=d&PlaySessionId=p HTTP/1.1"
    );

    let (base, _) =
        serve_once("HTTP/1.1 500 Oops\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert_eq!(
        UreqHttp::new(&base).delete("/x", "x").unwrap_err(),
        HttpError::Status(500)
    );
}

// ---- the background worker ---------------------------------------------

/// Calls `apply` until it returns a result, failing the test after a few seconds.
fn apply_soon(worker: &SyncWorker, store: &mut Store) -> Result<SyncReport, JellyfinError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = worker.apply(store, SYNCED_AT) {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker sent nothing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until `fetches` has counted at least `n`, failing after a few seconds.
fn wait_for_fetches(fetches: &AtomicUsize, n: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fetches.load(Ordering::SeqCst) < n {
        assert!(std::time::Instant::now() < deadline, "the worker stalled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// True for the request that starts a fetch: the first page of the item list.
fn starts_a_fetch(path: &str) -> bool {
    param(path, "Recursive") == Some("true")
        && param(path, "IncludeItemTypes") == Some("Movie,Episode")
        && param(path, "StartIndex") == Some("0")
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn the_worker_fetches_at_once_and_apply_writes_what_sync_would() {
    let (client, mut expected_store) = client_and_store(&FakeHttp::fixtures(500));
    let expected = sync(&client, &mut expected_store, SYNCED_AT).unwrap();
    let worker = SyncWorker::spawn(client_and_store(&FakeHttp::fixtures(500)).0, HOUR).unwrap();
    let mut store = Store::open_in_memory().unwrap();

    let report = apply_soon(&worker, &mut store).unwrap();

    assert_eq!(report, expected);
    assert_eq!(snapshot(&store), snapshot(&expected_store));
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT));
}

#[test]
fn a_failed_fetch_is_reported_by_apply_and_leaves_the_cache_untouched() {
    let mut store = Store::open_in_memory().unwrap();
    sync(&client(&FakeHttp::fixtures(500)), &mut store, SYNCED_AT - 1).unwrap();
    let before = snapshot(&store);
    let failing = FakeHttp::new(|_| Err(HttpError::Status(503)));
    let worker = SyncWorker::spawn(client(&failing), HOUR).unwrap();

    let err = apply_soon(&worker, &mut store).unwrap_err();

    assert_eq!(err, JellyfinError::Status(503));
    assert_eq!(snapshot(&store), before);
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT - 1));
}

#[test]
fn apply_with_nothing_pending_returns_none_and_writes_nothing() {
    // The fetch waits until the test drops `release`.
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let http = FakeHttp::new(move |path| {
        let _ = gate.lock().unwrap().recv();
        serve_fixtures(path, 500, ITEMS)
    });
    let worker = SyncWorker::spawn(client(&http), HOUR).unwrap();
    // Declared after `worker`, so a failed assertion drops it first and the
    // fetch can finish before the worker's drop joins the thread.
    let release = release;
    let mut store = Store::open_in_memory().unwrap();

    assert!(worker.apply(&mut store, SYNCED_AT).is_none());
    assert!(store.list_items().unwrap().is_empty());
    assert_eq!(store.library_synced_at_ms().unwrap(), None);

    drop(release);
    assert_eq!(apply_soon(&worker, &mut store).unwrap().items, 4);
}

#[test]
fn dropping_a_worker_does_not_wait_for_the_next_interval() {
    let worker = SyncWorker::spawn(client(&FakeHttp::fixtures(500)), HOUR).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    apply_soon(&worker, &mut store).unwrap();

    let started = std::time::Instant::now();
    drop(worker);

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "drop took {:?}",
        started.elapsed()
    );
}

#[test]
fn of_several_pending_snapshots_only_the_newest_is_written() {
    let only_movie_3 = {
        let all: Value = serde_json::from_str(ITEMS).unwrap();
        json!({ "Items": [all["Items"][2].clone()], "TotalRecordCount": 1 }).to_string()
    };
    // The first fetch sees the whole library; every later one sees only movie 3.
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = fetches.clone();
    let http = FakeHttp::new(move |path| {
        let n = if starts_a_fetch(path) {
            counter.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            counter.load(Ordering::SeqCst)
        };
        let items = if n == 1 { ITEMS } else { &only_movie_3 };
        serve_fixtures(path, 500, items)
    });
    let worker = SyncWorker::spawn(client(&http), Duration::from_millis(1)).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    // The third fetch starts only after the second was sent.
    wait_for_fetches(&fetches, 3);

    let report = apply_soon(&worker, &mut store).unwrap();

    assert_eq!(report.items, 1);
    assert_eq!(ids(&store.list_items().unwrap()), [MOVIE_3]);
}

#[test]
fn a_newer_failure_is_reported_but_the_newest_snapshot_is_still_written() {
    // The first fetch succeeds; every later one fails.
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = fetches.clone();
    let http = FakeHttp::new(move |path| {
        if starts_a_fetch(path) && counter.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(HttpError::Status(500));
        }
        serve_fixtures(path, 500, ITEMS)
    });
    let worker = SyncWorker::spawn(client(&http), Duration::from_millis(1)).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    wait_for_fetches(&fetches, 3);

    let err = apply_soon(&worker, &mut store).unwrap_err();

    assert_eq!(err, JellyfinError::Status(500));
    assert_eq!(store.list_items().unwrap().len(), 4);
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT));
}
