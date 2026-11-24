# Jellyfin source

`surfcult` reads its media library from Jellyfin and keeps a copy in SQLite. The code is in `src/jellyfin/`; the cache tables are in `migrations/0002_library_cache.sql`.

## How it works

`jellyfin::sync(&client, &mut store, now_ms)` fetches every playable movie and episode and every collection from Jellyfin, then replaces the cache in the `Store` in one transaction, recording `now_ms` as the sync time. The caller supplies the time; the sync step never reads the clock. `Store` implements the `MediaSource` trait, and `list_items`, `resolve` and `list_collections` read only the cache. They never call Jellyfin, so they keep working with the data of the last good sync when Jellyfin is down. `sync` fetches on the calling thread.

`jellyfin::SyncWorker::spawn(client, every)` fetches on a background thread named `jellyfin-sync` instead: once at start, then every `every`. It sends each result back over a channel and never touches the store, because `Store` is not `Sync`. The thread that owns the store calls `worker.apply(&mut store, now_ms)`, which does not block. It takes every pending result and writes only the newest successful snapshot, since each one is a whole library. It returns `None` when nothing is pending, the report of that write when the newest result is a snapshot, and the error when the newest result is a failed fetch; a snapshot that arrived before that failure is still written. A failed write leaves the cache as it was and its error is returned. Dropping the worker stops the thread without waiting for the next interval, though it does wait for a fetch in progress. If the worker thread panics, `apply` still writes what it sent before, then returns `None` and dropping the worker does not panic. Nothing creates a worker yet; the composition root (#23) will.

A sync that fails for any reason (server down, 401, a body that is not JSON, a failure while listing a collection, a cache write error) changes nothing. The cache keeps the previous library and `sync` returns an error. Before the first successful sync the cache is empty.

## Endpoints used

All requests are `GET`. `StartIndex` and `Limit` (500) page every list.

| What | Request |
|---|---|
| Items | `/Items?Recursive=true&IncludeItemTypes=Movie,Episode&Fields=MediaStreams,Chapters,Path,MediaSources&SortBy=SortName` |
| Collections | `/Items?Recursive=true&IncludeItemTypes=BoxSet&SortBy=SortName` |
| One collection's members | `/Items?ParentId=<collection id>&IncludeItemTypes=Movie,Episode` |

Paging follows `TotalRecordCount`. If a list ends before its count, the sync fails rather than caching a partial library. An item that appears on two pages is cached once.

## The API key

Jellyfin 12.4.0 accepts the key only in the header `Authorization: MediaBrowser Token="KEY"`. `X-Emby-Token` and an `api_key` query parameter return 401, so the client uses the header and nothing else. The key is never put in a URL or a command line.

`ApiKey::from_env()` reads the key from the first of these that is set and not empty:

1. `SURFCULT_JELLYFIN_API_KEY`, the key itself.
2. `SURFCULT_JELLYFIN_API_KEY_FILE`, the path of a file that holds the key. Whitespace around the key is trimmed. Prefer this form: the key stays out of the process environment listing, and the file can have mode 600.

`ApiKey` has no `Display`, and its `Debug` output is `ApiKey(<redacted>)`. Errors never contain it; if a transport's error text repeats it, the client replaces it with `[redacted]`. A key with a control character, a double quote or a backslash is refused because it would break the header.

The server's address is passed in as `JellyfinConfig { base_url, key }`. It is not in `channels.toml` yet. The HTTP client does not follow redirects, so the key cannot go to another host; a redirect is an error, and `base_url` must be the address Jellyfin answers on. Connecting takes at most 5 seconds and a whole request 30 seconds. Both `http` and `https` work; `https` uses rustls with built-in root certificates and needs no system library.

The requests carry no user id, so the key decides what is visible. Give surfcult a key that can read the libraries it schedules and cannot change anything.

## What is cached

For each kept item: id (the Jellyfin item id), title, kind (`movie` or `episode`), runtime, file path, chapters, video streams (codec, width, height, bit depth), audio streams (codec, channels, language) and the time of the sync.

- Jellyfin counts time in ticks of 100 ns. Runtime and chapter starts are stored in milliseconds, ticks divided by 10,000.
- Items come back in the order Jellyfin listed them (sorted by name). Chapters and streams keep their order.
- The file path is in the cache and in `Store::library_items()`, but not in `MediaItem`, which is unchanged. The path is for reconciling items by path when an id changes.
- Collections are Jellyfin `BoxSet` items. Each keeps its members in Jellyfin's order. A member that is not a cached item (a series, or an item that was skipped) is left out. `library::resolve_pool` turns a `jellyfin_collection` source into a channel's pool from the cached collection. Libraries, tags, genres and series are not resolved yet; that is #30.
- Playback URLs are not built here.

## What is skipped

A sync skips, and counts in `SyncReport::skipped`:

- Items with no video stream. Jellyfin lists a missing episode as an item with no streams; it cannot be played.
- Items with no runtime.
- Items whose JSON does not have the expected shape (for example a number sent as text).

Unknown fields never matter. Apart from an item's `Id`, every field may be missing or `null`: a video stream with no codec is cached as codec `unknown`, one with no size as 0 by 0, and an item with no name gets its id as the title.

## Tests

No test needs a Jellyfin server. The fixtures in `tests/fixtures/jellyfin/` are real Jellyfin 12.4.0 responses with fake names, paths and ids. Tests give the client a fake transport (the `JellyfinHttp` trait) that serves them. A few tests start a listener on `127.0.0.1` to check the real `ureq` transport (header sent, 401, redirect not followed, closed port); they do not leave the machine.
