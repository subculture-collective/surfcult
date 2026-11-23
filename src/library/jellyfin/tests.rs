//! Tests run against recorded Jellyfin 12.4.0 responses (names, paths and ids
//! are fake) and a fake transport. Only the last group opens a socket, and only
//! to a listener on 127.0.0.1 that the test starts itself.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;
use crate::library::{MediaId, MediaKind, MediaSource, SourceError};
use crate::store::Store;

const ITEMS: &str = include_str!("../../../tests/fixtures/jellyfin/items-page.json");
const BOXSETS: &str = include_str!("../../../tests/fixtures/jellyfin/boxsets-page.json");
const CHILDREN: &str =
    include_str!("../../../tests/fixtures/jellyfin/boxset-children-by-parent-id.json");

/// A value that stands for the API key. Tests look for it everywhere it must not be.
const SENTINEL: &str = "SENTINEL-KEY-0123456789abcdef";

fn key() -> ApiKey {
    ApiKey::new(SENTINEL).unwrap()
}

type Handler = Box<dyn Fn(&str) -> Result<String, HttpError> + Send + Sync>;

/// A transport that answers from a closure and records every request as
/// `(path_and_query, authorization)`.
struct FakeHttp {
    handler: Handler,
    requests: Mutex<Vec<(String, String)>>,
}

impl FakeHttp {
    fn new(
        handler: impl Fn(&str) -> Result<String, HttpError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Serves the recorded fixtures, at most `page_cap` items per page whatever
    /// the client asks for.
    fn fixtures(page_cap: usize) -> Arc<Self> {
        Self::new(move |path| serve_fixtures(path, page_cap, ITEMS))
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
    }

    fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|(path, _)| path).collect()
    }
}

impl JellyfinHttp for Arc<FakeHttp> {
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError> {
        self.requests
            .lock()
            .unwrap()
            .push((path_and_query.to_string(), authorization.to_string()));
        (self.handler)(path_and_query)
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

fn source_with(http: &Arc<FakeHttp>) -> JellyfinSource {
    JellyfinSource::new(
        JellyfinClient::with_transport(key(), http.clone()),
        Store::open_in_memory().unwrap(),
    )
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
    let source = source_with(&FakeHttp::fixtures(500));

    let report = source.sync().unwrap();

    assert_eq!(
        report,
        SyncReport {
            items: 4,
            collections: 2,
            skipped: 1
        }
    );
    let movie = source.resolve(&MediaId::new(MOVIE_1)).unwrap();
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

    let entries = source.cached_entries().unwrap();
    assert_eq!(
        entries[0].path.as_deref(),
        Some("/media/fixtures/movie-1.mkv")
    );
    assert!(source.last_synced_at_ms().unwrap().unwrap() > 0);
}

#[test]
fn episodes_and_extra_streams_are_read() {
    let source = source_with(&FakeHttp::fixtures(500));
    source.sync().unwrap();

    let episode = source.resolve(&MediaId::new(EPISODE_2)).unwrap();
    assert_eq!(episode.kind, MediaKind::Episode);
    assert_eq!(episode.runtime_ms, 1_436_105);
    assert_eq!(episode.audio[0].language.as_deref(), Some("jpn"));

    let movie_4 = source.resolve(&MediaId::new(MOVIE_4)).unwrap();
    let audio: Vec<_> = movie_4.audio.iter().map(|a| a.codec.as_str()).collect();
    assert_eq!(audio, ["truehd", "ac3"], "audio streams keep their order");

    let no_chapters = source.resolve(&MediaId::new(MOVIE_3)).unwrap();
    assert!(no_chapters.chapters.is_empty());
}

#[test]
fn a_placeholder_episode_with_no_streams_is_skipped() {
    let source = source_with(&FakeHttp::fixtures(500));
    source.sync().unwrap();

    let items = source.list_items().unwrap();

    assert_eq!(ids(&items), [MOVIE_1, EPISODE_2, MOVIE_3, MOVIE_4]);
    assert!(items.iter().all(|item| !item.title.contains("missing")));
}

#[test]
fn collections_list_their_members_in_jellyfins_order() {
    let source = source_with(&FakeHttp::fixtures(500));
    source.sync().unwrap();

    let collections = source.list_collections().unwrap();

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
    let source = source_with(&http);
    source.sync().unwrap();

    let members = &source.list_collections().unwrap()[0].members;

    assert_eq!(members, &[MediaId::new(MOVIE_1), MediaId::new(MOVIE_3)]);
}

#[test]
fn resolving_an_unknown_id_is_not_found() {
    let source = source_with(&FakeHttp::fixtures(500));
    source.sync().unwrap();

    let err = source.resolve(&MediaId::new("nope")).unwrap_err();

    assert_eq!(err, SourceError::NotFound(MediaId::new("nope")));
}

#[test]
fn before_any_sync_the_cache_is_empty() {
    let source = source_with(&FakeHttp::fixtures(500));

    assert!(source.list_items().unwrap().is_empty());
    assert!(source.list_collections().unwrap().is_empty());
    assert_eq!(source.last_synced_at_ms().unwrap(), None);
}

#[test]
fn syncing_again_replaces_items_that_went_away() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.db");
    source_with_store(FakeHttp::fixtures(500), Store::open(&db).unwrap())
        .sync()
        .unwrap();
    let only_movie_3 = {
        let all: Value = serde_json::from_str(ITEMS).unwrap();
        let movie_3 = all["Items"][2].clone();
        json!({ "Items": [movie_3], "TotalRecordCount": 1 }).to_string()
    };
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &only_movie_3));
    let source = source_with_store(http, Store::open(&db).unwrap());

    source.sync().unwrap();

    assert_eq!(ids(&source.list_items().unwrap()), [MOVIE_3]);
    // Collection A listed movies 1 and 3; only 3 remains in the cache.
    assert_eq!(
        source.list_collections().unwrap()[0].members,
        [MediaId::new(MOVIE_3)]
    );
}

// ---- paging -------------------------------------------------------------

#[test]
fn items_are_fetched_page_by_page() {
    let http = FakeHttp::fixtures(2);
    let source = source_with(&http);

    let report = source.sync().unwrap();

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
    let source = source_with(&http);

    let err = source.sync().unwrap_err();

    assert!(matches!(err, JellyfinError::BadResponse(_)), "{err}");
}

#[test]
fn a_response_without_a_count_stops_at_a_short_page() {
    let http = FakeHttp::new(|_| Ok(json!({ "Items": [] }).to_string()));
    let source = source_with(&http);

    let report = source.sync().unwrap();

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
    let source = source_with(&http);

    source.sync().unwrap();

    let a = source.resolve(&MediaId::new("a")).unwrap();
    assert_eq!(a.title, "a", "no Name falls back to the id");
    assert_eq!(a.runtime_ms, 600_000);
    assert_eq!(a.video[0].bit_depth, None);
    assert_eq!(a.audio[0].language, None);
    assert!(a.chapters.is_empty());
    let entries = source.cached_entries().unwrap();
    assert_eq!(
        entries[0].path.as_deref(),
        Some("/media/a.mkv"),
        "falls back to MediaSources"
    );
    let b = source.resolve(&MediaId::new("b")).unwrap();
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
    let source = source_with(&http);

    let report = source.sync().unwrap();

    assert_eq!(report.items, 1);
    assert_eq!(report.skipped, 5);
    assert_eq!(ids(&source.list_items().unwrap()), ["good"]);
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
    let source = source_with(&http);

    let report = source.sync().unwrap();

    assert_eq!(report.items, 1);
}

// ---- failure leaves the cache alone ------------------------------------

fn snapshot(
    source: &JellyfinSource,
) -> (
    Vec<crate::library::MediaItem>,
    Vec<crate::library::Collection>,
) {
    (
        source.list_items().unwrap(),
        source.list_collections().unwrap(),
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
    let good = source_with_store(FakeHttp::fixtures(500), Store::open(&db).unwrap());
    good.sync().unwrap();
    let before = snapshot(&good);
    let synced_at = good.last_synced_at_ms().unwrap();
    drop(good);

    let broken = source_with_store(FakeHttp::new(failing), Store::open(&db).unwrap());
    let err = broken.sync().unwrap_err();

    assert!(expected(&err), "unexpected error: {err}");
    assert_eq!(snapshot(&broken), before);
    assert_eq!(broken.last_synced_at_ms().unwrap(), synced_at);
}

fn source_with_store(http: Arc<FakeHttp>, store: Store) -> JellyfinSource {
    JellyfinSource::new(JellyfinClient::with_transport(key(), http), store)
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
    source_with_store(FakeHttp::fixtures(500), Store::open(&db).unwrap())
        .sync()
        .unwrap();

    // A later start: the server is down, and nothing is asked of it for reads.
    let http = FakeHttp::new(|_| Err(HttpError::Unreachable("no route to host".into())));
    let source = source_with_store(http.clone(), Store::open(&db).unwrap());

    assert_eq!(source.list_items().unwrap().len(), 4);
    assert_eq!(
        source.resolve(&MediaId::new(MOVIE_1)).unwrap().runtime_ms,
        5_823_840
    );
    assert_eq!(source.list_collections().unwrap().len(), 2);
    assert!(http.requests().is_empty(), "reads never go to Jellyfin");
    assert!(source.sync().is_err());
}

// ---- the API key --------------------------------------------------------

#[test]
fn the_key_is_sent_only_in_the_authorization_header() {
    let http = FakeHttp::fixtures(2);
    let source = source_with(&http);

    source.sync().unwrap();

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
    let source = source_with(&http);
    source.sync().unwrap();

    let mut texts = vec![
        format!("{:?}", key()),
        format!("{client:?}"),
        format!("{config:?}"),
        format!("{source:?}"),
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
    let err = source_with(&leaky).sync().unwrap_err();
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
    texts.push(source_with(&unauthorized).sync().unwrap_err().to_string());
    let malformed = FakeHttp::new(|_| Ok(format!("not json {SENTINEL}")));
    let err = source_with(&malformed).sync().unwrap_err();
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

// ---- the real transport, against a listener this test starts ------------

/// Accepts one connection, returns the raw request text and answers with `reply`.
fn serve_once(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
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
