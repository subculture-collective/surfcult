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
use crate::library::{
    LibraryChanges, MediaId, MediaKind, MediaSource, Plays, SeriesEpisode, SourceError,
};
use crate::store::Store;

const ITEMS: &str = include_str!("../../tests/fixtures/jellyfin/items-page.json");
const BOXSETS: &str = include_str!("../../tests/fixtures/jellyfin/boxsets-page.json");
const CHILDREN: &str =
    include_str!("../../tests/fixtures/jellyfin/boxset-children-by-parent-id.json");
const SERIES: &str = include_str!("../../tests/fixtures/jellyfin/series-page.json");
const MEDIA_FOLDERS: &str = include_str!("../../tests/fixtures/jellyfin/media-folders.json");
const LIBRARY_CHILDREN: &str =
    include_str!("../../tests/fixtures/jellyfin/library-children-by-parent-id.json");
/// Two `MusicVideo` items, written by hand in the shape Jellyfin sends.
const MUSIC_VIDEOS: &str = include_str!("../../tests/fixtures/jellyfin/music-videos-page.json");

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
            handler: Box::new(|path| {
                Ok(match path {
                    "/Users" => json!([
                        { "Id": USER_ID, "Name": "owner", "Policy": { "IsDisabled": false } }
                    ])
                    .to_string(),
                    _ => String::new(),
                })
            }),
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
        // Libraries are listed recursively, collections are not.
        let fixture = if param(path, "Recursive").is_some() {
            LIBRARY_CHILDREN
        } else {
            CHILDREN
        };
        let children: Value = serde_json::from_str(fixture).unwrap();
        let all = children[parent]["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        return Ok(page(&all, path, cap));
    }
    if path.starts_with("/Library/MediaFolders?") {
        return Ok(page(&list(MEDIA_FOLDERS), path, cap));
    }
    match param(path, "IncludeItemTypes") {
        Some("BoxSet") => Ok(page(&list(BOXSETS), path, cap)),
        Some("Series") => Ok(page(&list(SERIES), path, cap)),
        _ => Ok(page(&list(items), path, cap)),
    }
}

/// When the tests say a sync happened, in Unix epoch milliseconds.
const SYNCED_AT: i64 = 1_767_225_600_000;
/// The id of the one user the fake Jellyfin lists.
const USER_ID: &str = "07a5e934d9c04bc5b9c4c2a7e0c2a8f1";

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
const SERIES_ID: &str = "5e1e5000-0000-4000-8000-000000000001";

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
            skipped: 1,
            changes: LibraryChanges::default(),
            written: true,
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
    assert_eq!(
        collections[1].members,
        [MediaId::new(MOVIE_4), MediaId::new(SERIES_ID)],
        "a series is kept by its id"
    );
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
fn a_sync_reads_series_tags_genres_and_libraries() {
    let http = FakeHttp::fixtures(500);
    let (client, mut store) = client_and_store(&http);
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let episode = store.resolve(&MediaId::new(EPISODE_2)).unwrap();
    assert_eq!(
        episode.series,
        Some(SeriesEpisode {
            series_id: MediaId::new(SERIES_ID),
            series_name: "Fixture Series".into(),
            season: Some(1),
            number: Some(2),
        })
    );
    assert_eq!(episode.tags, ["Saturday Morning"], "from the series");
    assert_eq!(episode.genres, ["Animation", "Comedy"], "from the series");
    assert_eq!(episode.libraries, ["Fixture Shows"]);

    let movie = store.resolve(&MediaId::new(MOVIE_1)).unwrap();
    assert_eq!(movie.series, None);
    assert_eq!(movie.tags, ["Fixture Tag"]);
    assert_eq!(movie.genres, ["Drama", "Comedy"]);
    assert_eq!(movie.libraries, ["Fixture Movies"]);

    let paths = http.paths();
    assert!(paths.iter().any(|p| {
        p.starts_with("/Items?Recursive=true&IncludeItemTypes=Movie,Episode,MusicVideo")
            && p.contains("Fields=MediaStreams,Chapters,Path,MediaSources,Tags,Genres")
    }));
    assert!(paths.iter().any(|p| {
        p.starts_with("/Items?Recursive=true&IncludeItemTypes=Series&Fields=Tags,Genres")
    }));
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/Library/MediaFolders?IsHidden=false"))
    );
    let library_pages: Vec<_> = paths
        .iter()
        .filter(|p| param(p, "ParentId").is_some() && param(p, "Recursive").is_some())
        .collect();
    assert!(!library_pages.is_empty());
    assert!(
        library_pages
            .iter()
            .all(|p| param(p, "SortBy") == Some("SortName")),
        "library pages are sorted so paging is stable: {library_pages:?}"
    );
}

#[test]
fn an_episodes_own_labels_come_first_and_series_labels_are_not_repeated() {
    let items = json!({
        "Items": [{
            "Id": "e", "Type": "Episode", "RunTimeTicks": 10_000_000i64,
            "SeriesId": SERIES_ID.replace('-', ""), "SeriesName": "Fixture Series",
            "Tags": ["Own", "Saturday Morning", ""], "Genres": null,
            "MediaStreams": [{ "Type": "Video", "Codec": "h264", "Width": 1, "Height": 1 }]
        }],
        "TotalRecordCount": 1
    })
    .to_string();
    let http = FakeHttp::new(move |path| serve_fixtures(path, 500, &items));
    let (client, mut store) = client_and_store(&http);
    sync(&client, &mut store, SYNCED_AT).unwrap();

    let episode = store.resolve(&MediaId::new("e")).unwrap();

    assert_eq!(episode.tags, ["Own", "Saturday Morning"]);
    assert_eq!(episode.genres, ["Animation", "Comedy"]);
    let series = episode.series.unwrap();
    assert_eq!(series.series_id.as_str(), SERIES_ID.replace('-', ""));
    assert_eq!((series.season, series.number), (None, None));
}

#[test]
fn every_source_kind_resolves_from_a_synced_cache() {
    use crate::config::Source;
    use crate::library::{PoolErrorReason, resolve_pool};

    let (client, mut store) = client_and_store(&FakeHttp::fixtures(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();
    let resolve = |source: Source| resolve_pool(7, Plays::Videos, &source, &store);
    let pool = |source: Source| ids(&resolve(source).unwrap()).join(" ");

    // Collection B lists movie 4 and the series, which becomes its episode.
    assert_eq!(
        pool(Source::JellyfinCollection("Fixture Collection B".into())),
        [MOVIE_4, EPISODE_2].join(" ")
    );
    assert_eq!(
        pool(Source::JellyfinLibrary("Fixture Movies".into())),
        [MOVIE_1, MOVIE_3, MOVIE_4].join(" ")
    );
    // Movie 4's tag is "fixture tag"; names match exactly.
    assert_eq!(pool(Source::JellyfinTag("Fixture Tag".into())), MOVIE_1);
    assert_eq!(
        pool(Source::JellyfinTag("Saturday Morning".into())),
        EPISODE_2
    );
    assert_eq!(
        pool(Source::JellyfinGenre("Comedy".into())),
        [MOVIE_1, EPISODE_2, MOVIE_3].join(" ")
    );
    assert_eq!(
        pool(Source::JellyfinSeries("Fixture Series".into())),
        EPISODE_2
    );

    let err = resolve(Source::JellyfinGenre("Western".into())).unwrap_err();
    assert_eq!(err.reason, PoolErrorReason::NotFound);
    assert_eq!(
        err.to_string(),
        "channel 7: jellyfin_genre \"Western\" matches nothing in the library"
    );
}

const VIDEO_1: &str = "6d0c1e55-0000-4000-8000-000000000001";
const VIDEO_2: &str = "6d0c1e55-0000-4000-8000-000000000002";
const MUSIC_VIDEOS_LIBRARY: &str = "55555555-5555-5555-5555-555555555555";

/// The recorded fixtures plus a `Music Videos` library holding the two
/// music videos in [`MUSIC_VIDEOS`].
fn with_music_videos() -> Arc<FakeHttp> {
    let list = |body: &str| serde_json::from_str::<Value>(body).unwrap()["Items"].clone();
    let mut items = list(ITEMS).as_array().unwrap().clone();
    items.extend(list(MUSIC_VIDEOS).as_array().unwrap().iter().cloned());
    let items = json!({ "Items": items, "TotalRecordCount": items.len() }).to_string();
    FakeHttp::new(move |path| {
        if param(path, "ParentId") == Some(MUSIC_VIDEOS_LIBRARY) {
            let children = [VIDEO_1, VIDEO_2].map(|id| json!({ "Id": id, "Type": "MusicVideo" }));
            return Ok(page(&children, path, 500));
        }
        if path.starts_with("/Library/MediaFolders?") {
            let mut folders = list(MEDIA_FOLDERS).as_array().unwrap().clone();
            folders.push(json!({
                "Name": "Music Videos", "Id": MUSIC_VIDEOS_LIBRARY,
                "Type": "CollectionFolder", "CollectionType": "musicvideos", "IsFolder": true
            }));
            return Ok(page(&folders, path, 500));
        }
        serve_fixtures(path, 500, &items)
    })
}

#[test]
fn a_sync_caches_music_videos_with_their_artists_album_and_genres() {
    let http = with_music_videos();
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    assert_eq!((report.items, report.skipped), (6, 1));
    let video = store.resolve(&MediaId::new(VIDEO_1)).unwrap();
    assert_eq!(video.kind, MediaKind::MusicVideo);
    assert_eq!(video.title, "Fixture Song");
    assert_eq!(video.artists, ["Fixture Artist", "Guest Artist"]);
    assert_eq!(video.album.as_deref(), Some("Fixture Album"));
    assert_eq!(video.genres, ["Alternative", "Rock"]);
    assert_eq!(video.tags, ["Official Video"]);
    assert_eq!(video.libraries, ["Music Videos"]);
    assert_eq!(video.runtime_ms, 241_200);
    assert_eq!(video.series, None);
    assert_eq!(video.video[0].codec, "h264");
    assert_eq!(
        video.display_title(),
        "Fixture Artist, Guest Artist – Fixture Song"
    );

    let bare = store.resolve(&MediaId::new(VIDEO_2)).unwrap();
    assert!(bare.artists.is_empty());
    assert_eq!(bare.album, None);
    assert_eq!(bare.display_title(), "Fixture Instrumental");

    // The sync asks for music videos everywhere it asks for movies.
    let paths = http.paths();
    for path in paths.iter().filter(|p| p.contains("Movie")) {
        assert!(path.contains("Movie,Episode,MusicVideo"), "{path}");
    }
}

#[test]
fn music_video_sources_resolve_from_a_synced_cache() {
    use crate::config::Source;
    use crate::library::resolve_pool;

    let (client, mut store) = client_and_store(&with_music_videos());
    sync(&client, &mut store, SYNCED_AT).unwrap();
    let pool =
        |source: Source| ids(&resolve_pool(7, Plays::Videos, &source, &store).unwrap()).join(" ");

    assert_eq!(
        pool(Source::JellyfinLibrary("Music Videos".into())),
        [VIDEO_1, VIDEO_2].join(" ")
    );
    assert_eq!(pool(Source::JellyfinGenre("Hip-Hop".into())), VIDEO_2);
    assert_eq!(pool(Source::JellyfinGenre("Rock".into())), VIDEO_1);
    assert_eq!(pool(Source::JellyfinTag("Official Video".into())), VIDEO_1);
}

const SONG_1: &str = "50a90000-0000-4000-8000-000000000001";
const SONG_2: &str = "50a90000-0000-4000-8000-000000000002";
const MUSIC_LIBRARY: &str = "66666666-6666-6666-6666-666666666666";
/// Three `Audio` items, written by hand in the shape Jellyfin sends. The
/// third has no runtime.
const SONGS: &str = include_str!("../../tests/fixtures/jellyfin/songs-page.json");

/// The recorded fixtures plus a `Music` library holding the songs in
/// [`SONGS`], at most `cap` items per page.
fn with_songs(cap: usize) -> Arc<FakeHttp> {
    let list = |body: &str| serde_json::from_str::<Value>(body).unwrap()["Items"].clone();
    FakeHttp::new(move |path| {
        if param(path, "ParentId") == Some(MUSIC_LIBRARY) {
            let items = match param(path, "IncludeItemTypes") {
                Some("Audio") => list(SONGS).as_array().unwrap().clone(),
                _ => Vec::new(),
            };
            return Ok(page(&items, path, cap));
        }
        if path.starts_with("/Library/MediaFolders?") {
            let mut folders = list(MEDIA_FOLDERS).as_array().unwrap().clone();
            folders.push(json!({
                "Name": "Music", "Id": MUSIC_LIBRARY,
                "Type": "CollectionFolder", "CollectionType": "music", "IsFolder": true
            }));
            return Ok(page(&folders, path, cap));
        }
        serve_fixtures(path, cap, ITEMS)
    })
}

#[test]
fn a_sync_caches_the_songs_of_a_music_library() {
    let http = with_songs(2);
    let (client, mut store) = client_and_store(&http);

    let report = sync(&client, &mut store, SYNCED_AT).unwrap();

    // Four videos, two songs; a placeholder episode and the song without a
    // runtime are skipped.
    assert_eq!((report.items, report.skipped), (6, 2));
    let song = store.resolve(&MediaId::new(SONG_1)).unwrap();
    assert_eq!(song.kind, MediaKind::Song);
    assert_eq!(song.title, "Fixture Anthem");
    assert_eq!(song.artists, ["Fixture Band"]);
    assert_eq!(song.album.as_deref(), Some("Fixture Record"));
    assert_eq!(
        song.album_id,
        Some(MediaId::new("a1b00000-0000-4000-8000-000000000001"))
    );
    assert_eq!(song.genres, ["Punk", "Comedy"]);
    assert_eq!(song.libraries, ["Music"]);
    assert_eq!(song.runtime_ms, 205_100);
    assert!(song.video.is_empty(), "the cover is not a picture");
    assert_eq!(song.audio[0].codec, "flac");
    assert_eq!(song.display_title(), "Fixture Band – Fixture Anthem");
    let entries = store.library_items().unwrap();
    let path = entries
        .iter()
        .find(|e| e.item.id.as_str() == SONG_1)
        .unwrap();
    assert_eq!(
        path.path.as_deref(),
        Some("/media/fixtures/Music/Fixture Band/Fixture Record/01 - Fixture Anthem.flac")
    );

    let single = store.resolve(&MediaId::new(SONG_2)).unwrap();
    assert_eq!(single.album, None);
    assert_eq!(single.album_id, None);

    // Songs are asked for once per music library, page by page, without
    // images or user data.
    let song_pages: Vec<String> = http
        .paths()
        .into_iter()
        .filter(|p| param(p, "IncludeItemTypes") == Some("Audio"))
        .collect();
    let starts: Vec<_> = song_pages
        .iter()
        .map(|p| param(p, "StartIndex").unwrap())
        .collect();
    assert_eq!(starts, ["0", "2"]);
    for p in &song_pages {
        assert_eq!(param(p, "ParentId"), Some(MUSIC_LIBRARY), "{p}");
        assert_eq!(param(p, "Fields"), Some("Path,Genres,MediaStreams"), "{p}");
        assert_eq!(param(p, "EnableImages"), Some("false"), "{p}");
    }
    // No other request asks for songs.
    assert!(
        http.paths()
            .iter()
            .filter(|p| p.contains("Audio"))
            .all(|p| param(p, "ParentId") == Some(MUSIC_LIBRARY))
    );
}

#[test]
fn songs_reach_only_channels_that_play_songs() {
    use crate::config::Source;
    use crate::library::resolve_pool;

    let (client, mut store) = client_and_store(&with_songs(500));
    sync(&client, &mut store, SYNCED_AT).unwrap();
    let pool = |plays, source: Source| {
        resolve_pool(7, plays, &source, &store)
            .map(|items| ids(&items).join(" "))
            .map_err(|e| e.to_string())
    };
    let comedy = || Source::JellyfinGenre("Comedy".into());

    // Movie 1 and the song share "Comedy"; each kind of channel gets its own.
    assert_eq!(
        pool(Plays::Videos, comedy()).unwrap(),
        [MOVIE_1, EPISODE_2, MOVIE_3].join(" ")
    );
    assert_eq!(pool(Plays::Songs, comedy()).unwrap(), SONG_1);
    assert_eq!(
        pool(Plays::Songs, Source::JellyfinGenre("Punk".into())).unwrap(),
        [SONG_1, SONG_2].join(" ")
    );
    assert_eq!(
        pool(Plays::Videos, Source::JellyfinGenre("Punk".into())).unwrap_err(),
        "channel 7: jellyfin_genre \"Punk\" matches nothing in the library"
    );
    assert_eq!(
        pool(
            Plays::Songs,
            Source::JellyfinLibrary("Fixture Movies".into())
        )
        .unwrap_err(),
        "channel 7: jellyfin_library \"Fixture Movies\" matches no song in the library"
    );
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
        .filter(|path| path.contains("IncludeItemTypes=Movie,Episode,MusicVideo&Fields="))
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
        4,
        "one request each for items, series, libraries and collections"
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
fn a_failure_while_listing_series_or_libraries_leaves_the_cache_untouched() {
    assert_failed_sync_keeps_cache(
        |path| {
            if param(path, "IncludeItemTypes") == Some("Series") {
                Err(HttpError::Status(500))
            } else {
                serve_fixtures(path, 500, ITEMS)
            }
        },
        |err| *err == JellyfinError::Status(500),
    );
    assert_failed_sync_keeps_cache(
        |path| {
            if param(path, "ParentId").is_some() && param(path, "Recursive").is_some() {
                Err(HttpError::Status(502))
            } else {
                serve_fixtures(path, 500, ITEMS)
            }
        },
        |err| *err == JellyfinError::Status(502),
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
        user_id: None,
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
        // Jellyfin 12.4.0 answers 400 to a device profile without a user
        // (found on the Pi, 2026-11-24).
        if !request["DeviceProfile"].is_null() && request["UserId"].as_str() != Some(USER_ID) {
            return Err(HttpError::Status(400));
        }
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
        // Jellyfin 12.4.0 transcodes an item with subtitles when the profile
        // offers no subtitle format, whatever its video and audio are.
        let subtitles_offered = request["DeviceProfile"]["SubtitleProfiles"]
            .as_array()
            .is_some_and(|formats| !formats.is_empty());
        let info =
            if subtitles_offered && jellyfin_direct_plays(&request["DeviceProfile"], &streams) {
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
fn a_song_gets_an_audio_direct_play_url() {
    let http = FakeHttp::with_post(|_, _| {
        Ok(json!({
            "MediaSources": [{
                "Id": "50a9000000004000800000000000000a",
                "SupportsDirectPlay": true,
                "MediaStreams": [
                    { "Type": "Audio", "Codec": "flac", "Channels": 2 },
                    { "Type": "EmbeddedImage", "Codec": "mjpeg" }
                ]
            }],
            "PlaySessionId": "session"
        })
        .to_string())
    });

    let playback = client(&http)
        .playback(&MediaId::new(SONG_1), 30_000)
        .unwrap();

    assert_eq!(
        playback,
        Playback::Direct {
            path_and_query: format!(
                "/Audio/{SONG_1}/stream?static=true&mediaSourceId=50a9000000004000800000000000000a"
            ),
            seek_ms: 30_000,
        }
    );
    let profile = super::playback::device_profile();
    let audio = &profile["DirectPlayProfiles"][1];
    assert_eq!(audio["Type"], "Audio");
    assert!(audio.get("Container").is_none(), "any container plays");
    let codecs: Vec<&str> = audio["AudioCodec"].as_str().unwrap().split(',').collect();
    assert!(
        codecs.contains(&"flac") && codecs.contains(&"mp3"),
        "{codecs:?}"
    );
}

#[test]
fn an_album_cover_is_asked_for_as_a_scaled_png() {
    let http = FakeHttp::new(|_| Ok("png bytes".into()));

    let bytes = client(&http)
        .primary_image(&MediaId::new("a1b0 0000"), 440)
        .unwrap();

    assert_eq!(bytes, b"png bytes");
    let calls = http.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "GET");
    assert_eq!(
        calls[0].path,
        "/Items/a1b0%200000/Images/Primary?format=Png&maxWidth=440&maxHeight=440"
    );
    assert_eq!(calls[0].authorization, key().authorization_header());
    let missing = FakeHttp::new(|_| Err(HttpError::Status(404)));
    assert_eq!(
        client(&missing).primary_image(&MediaId::new("x"), 440),
        Err(JellyfinError::Status(404))
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
    assert_eq!(calls.len(), 2, "the user lookup, then the request");
    assert_eq!((calls[0].method, calls[0].path.as_str()), ("GET", "/Users"));
    assert_eq!(calls[1].method, "POST");
    assert_eq!(calls[1].path, format!("/Items/{MOVIE_1}/PlaybackInfo"));
    let body: Value = serde_json::from_str(calls[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["UserId"], USER_ID);
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
        && param(path, "ParentId").is_none()
        && param(path, "IncludeItemTypes") == Some("Movie,Episode,MusicVideo")
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
fn an_unchanged_library_is_not_written_again_but_its_sync_time_moves() {
    let worker =
        SyncWorker::spawn(client(&FakeHttp::fixtures(500)), Duration::from_millis(1)).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let first = apply_soon(&worker, &mut store).unwrap();
    assert!(first.written);
    // Something else changes the cache; the worker does not know.
    store.replace_library(&[], &[], 5).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let again = loop {
        assert!(std::time::Instant::now() < deadline, "no second fetch");
        if let Some(result) = worker.apply(&mut store, SYNCED_AT + 9) {
            break result.unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    assert!(!again.written);
    assert_eq!(
        (again.items, again.collections),
        (first.items, first.collections)
    );
    assert_eq!(again.changes, LibraryChanges::default());
    assert!(store.list_items().unwrap().is_empty(), "not written again");
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(SYNCED_AT + 9));
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
fn dropping_a_worker_does_not_wait_for_a_fetch_in_progress() {
    // The fetch blocks for up to 5 s, so a drop that waits for it fails the
    // assertion instead of hanging the test.
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let http = FakeHttp::new(move |path| {
        let _ = started_tx.lock().unwrap().send(());
        let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
        serve_fixtures(path, 500, ITEMS)
    });
    let worker = SyncWorker::spawn(client(&http), HOUR).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let started = std::time::Instant::now();
    drop(worker);

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "drop took {:?}",
        started.elapsed()
    );
    drop(release);
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

#[test]
fn playback_is_requested_for_the_first_enabled_user_and_the_lookup_is_kept() {
    let http = fake_jellyfin();
    let client = client(&http);

    client.playback(&MediaId::new(MOVIE_1), 0).unwrap();
    client.playback(&MediaId::new(MOVIE_1), 60_000).unwrap();

    let paths = http.paths();
    assert_eq!(
        paths.iter().filter(|path| *path == "/Users").count(),
        1,
        "{paths:?}"
    );
    let bodies: Vec<Value> = http
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|call| call.body.as_deref())
        .map(|body| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies.iter().all(|body| body["UserId"] == USER_ID));
}

#[test]
fn a_configured_user_id_is_used_without_listing_users() {
    let http = fake_jellyfin();
    let client = JellyfinClient::with_transport(key(), http.clone()).with_user_id(USER_ID);

    client.playback(&MediaId::new(MOVIE_1), 0).unwrap();

    assert!(!http.paths().iter().any(|path| path == "/Users"));
}

#[test]
fn disabled_users_are_skipped_and_no_enabled_user_is_an_error() {
    let users = |body: Value| {
        let http = FakeHttp::new(move |path| {
            assert_eq!(path, "/Users");
            Ok(body.to_string())
        });
        client(&http).user_id()
    };

    let id = users(json!([
        { "Id": "aaaa", "Policy": { "IsDisabled": true } },
        { "Id": "bbbb", "Policy": { "IsDisabled": false } },
    ]))
    .unwrap();
    assert_eq!(id, "bbbb");

    let err = users(json!([{ "Id": "aaaa", "Policy": { "IsDisabled": true } }])).unwrap_err();
    assert!(matches!(err, JellyfinError::NoPlayback(_)), "{err:?}");
    let err = users(json!([])).unwrap_err();
    assert!(matches!(err, JellyfinError::NoPlayback(_)), "{err:?}");
}

#[test]
fn the_profile_offers_every_subtitle_format_as_external() {
    // Without them Jellyfin 12.4.0 transcodes any item that has subtitles.
    let profile = super::playback::device_profile();
    let formats = profile["SubtitleProfiles"].as_array().unwrap();
    assert!(formats.len() >= 10, "{formats:?}");
    assert!(formats.iter().all(|format| format["Method"] == "External"));
    for needed in ["srt", "ass", "pgssub"] {
        assert!(
            formats.iter().any(|format| format["Format"] == needed),
            "{needed}"
        );
    }
}

// ---- cost ---------------------------------------------------------------

/// What a sync of a music library the size of the owner's (about 32,000
/// songs) costs: run with
/// `cargo test --release -- --ignored --nocapture sync_cost` and read the
/// printed times. Peak memory is the process's, so measure it from outside
/// (`/usr/bin/time -v`). docs/reference/jellyfin.md records the results.
#[test]
#[ignore = "a measurement, not a check; see docs/reference/jellyfin.md"]
fn sync_cost_of_a_music_library() {
    use std::time::Instant;

    use crate::config::Source;
    use crate::library::resolve_pool;

    const SONGS: usize = 32_000;
    const GENRES: [&str; 8] = [
        "Punk",
        "Rock",
        "Hip Hop",
        "Electronic",
        "Jazz",
        "Indie Rock",
        "Ska",
        "Ambient",
    ];
    let song = |i: usize| {
        json!({
            "Name": format!("Song number {i}"),
            "ServerId": "00000000000000000000000000000000",
            "Id": format!("{i:032x}"),
            "RunTimeTicks": 2_400_000_000i64 + (i as i64 % 900) * 1_000_000,
            "Container": "flac",
            "Path": format!("/media/music/Artist {a}/Album {b}/{i:02} - Song number {i}.flac",
                a = i / 32, b = i / 12),
            "Type": "Audio", "IsFolder": false, "MediaType": "Audio",
            "LocationType": "FileSystem",
            "IndexNumber": i % 12, "ParentIndexNumber": 1,
            "Album": format!("Album {}", i / 12),
            "AlbumId": format!("{:032x}", 1_000_000 + i / 12),
            "AlbumArtist": format!("Artist {}", i / 32),
            "Artists": [format!("Artist {}", i / 32)],
            "ArtistItems": [{ "Name": format!("Artist {}", i / 32), "Id": format!("{:032x}", 2_000_000 + i / 32) }],
            "MediaStreams": [
                { "Type": "Audio", "Codec": "flac", "IsDefault": true, "Index": 0,
                  "Channels": 2, "ChannelLayout": "stereo", "SampleRate": 44100, "BitDepth": 16,
                  "BitRate": 900_000, "DisplayTitle": "FLAC - Stereo" },
                { "Type": "EmbeddedImage", "Codec": "mjpeg", "Index": 1, "Width": 600, "Height": 600 }
            ],
            "Genres": [GENRES[i % GENRES.len()], GENRES[(i / 7) % GENRES.len()]]
        })
    };
    let http = FakeHttp::new(move |path| {
        if param(path, "ParentId") == Some(MUSIC_LIBRARY) {
            if param(path, "IncludeItemTypes") != Some("Audio") {
                return Ok(page(&[], path, 500));
            }
            let start: usize = param(path, "StartIndex").unwrap().parse().unwrap();
            let items: Vec<Value> = (start..SONGS.min(start + 500)).map(song).collect();
            return Ok(json!({ "Items": items, "TotalRecordCount": SONGS }).to_string());
        }
        if path.starts_with("/Library/MediaFolders?") {
            let folders = [json!({
                "Name": "Music", "Id": MUSIC_LIBRARY,
                "Type": "CollectionFolder", "CollectionType": "music", "IsFolder": true
            })];
            return Ok(page(&folders, path, 500));
        }
        serve_fixtures(path, 500, ITEMS)
    });
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("surfcult.db")).unwrap();
    let client = client(&http);

    let started = Instant::now();
    let fetched = client.fetch_library().unwrap();
    let fetch = started.elapsed();
    assert_eq!(fetched.entries.len(), SONGS + 4);
    let started = Instant::now();
    super::sync::write_fetched(&mut store, &fetched, SYNCED_AT).unwrap();
    let write = started.elapsed();
    drop(fetched);
    let started = Instant::now();
    let again = client.fetch_library().unwrap();
    super::sync::write_fetched(&mut store, &again, SYNCED_AT + 1).unwrap();
    let resync = started.elapsed();
    drop(again);

    let started = Instant::now();
    let punk = resolve_pool(
        7,
        Plays::Songs,
        &Source::JellyfinGenre("Punk".into()),
        &store,
    )
    .unwrap();
    let genre = started.elapsed();
    let started = Instant::now();
    let comedy = resolve_pool(
        7,
        Plays::Videos,
        &Source::JellyfinGenre("Comedy".into()),
        &store,
    )
    .unwrap();
    let video = started.elapsed();
    let started = Instant::now();
    let all = store.list_items().unwrap();
    let everything = started.elapsed();
    let bytes = std::fs::metadata(dir.path().join("surfcult.db"))
        .unwrap()
        .len();

    assert_eq!(all.len(), SONGS + 4);
    assert_eq!(comedy.len(), 3);
    println!(
        "{SONGS} songs: fetch and parse {fetch:?}, write {write:?}, \
         fetch and replace again {resync:?}; one genre ({} songs) {genre:?}, \
         a video genre {video:?}, the whole cache {everything:?}; database {} KiB",
        punk.len(),
        bytes / 1024
    );
}
