# Jellyfin source

`surfcult` reads its media library from Jellyfin and keeps a copy in SQLite. The code is in `src/jellyfin/`; the cache tables are in `migrations/0002_library_cache.sql` and later migrations.

## How it works

`jellyfin::sync(&client, &mut store, now_ms)` fetches every playable movie, episode and music video, the songs of every music library, and every collection from Jellyfin, then replaces the cache in the `Store` in one transaction, recording `now_ms` as the sync time. The caller supplies the time; the sync step never reads the clock. `Store` implements the `MediaSource` trait, and `list_items`, `resolve` and `list_collections` read only the cache. They never call Jellyfin, so they keep working with the data of the last good sync when Jellyfin is down. `sync` fetches on the calling thread.

`jellyfin::SyncWorker::spawn(client, every)` fetches on a background thread named `jellyfin-sync` instead: once at start, then every `every`. It sends each result back over a channel and never touches the store, because `Store` is not `Sync`. The thread that owns the store calls `worker.apply(&mut store, now_ms)`, which does not block. It takes every pending result and writes only the newest successful snapshot, since each one is a whole library. A snapshot that matches the one the worker last wrote is not written again: the worker thread hashes each snapshot (`FetchedLibrary::fingerprint`), and when the hash is the same, `apply` only records the sync time (`Store::mark_library_synced`) and reports `written: false`. A music library makes the cache large, and writing it costs seconds on the Pi (see [Sync cost](#sync-cost)); most syncs find nothing new. The first sync after the process starts always writes. It returns `None` when nothing is pending, the report of that write when the newest result is a snapshot, and the error when the newest result is a failed fetch; a snapshot that arrived before that failure is still written. A failed write leaves the cache as it was and its error is returned. Dropping the worker stops the thread without waiting for the next interval, though it does wait for a fetch in progress. If the worker thread panics, `apply` still writes what it sent before, then returns `None` and dropping the worker does not panic. `surfcult run` creates one ([run.md](run.md)).

A sync that fails for any reason (server down, 401, a body that is not JSON, a failure while listing a collection, a cache write error) changes nothing. The cache keeps the previous library and `sync` returns an error. Before the first successful sync the cache is empty.

## Endpoints used

The sync sends only `GET` requests. `StartIndex` and `Limit` (500) page every list.

| What | Request |
|---|---|
| Items | `GET /Items?Recursive=true&IncludeItemTypes=Movie,Episode,MusicVideo&Fields=MediaStreams,Chapters,Path,MediaSources,Tags,Genres&SortBy=SortName` |
| Series, for their tags and genres | `GET /Items?Recursive=true&IncludeItemTypes=Series&Fields=Tags,Genres&SortBy=SortName` |
| Libraries | `GET /Library/MediaFolders?IsHidden=false` |
| One library's items | `GET /Items?ParentId=<library id>&Recursive=true&IncludeItemTypes=Movie,Episode,MusicVideo&SortBy=SortName` |
| One music library's songs | `GET /Items?ParentId=<library id>&Recursive=true&IncludeItemTypes=Audio&Fields=Path,Genres,MediaStreams&EnableImages=false&EnableUserData=false&SortBy=AlbumArtist,Album,ParentIndexNumber,IndexNumber,SortName` |
| Collections | `GET /Items?Recursive=true&IncludeItemTypes=BoxSet&SortBy=SortName` |
| One collection's members | `GET /Items?ParentId=<collection id>&IncludeItemTypes=Movie,Episode,MusicVideo,Series` |
| How to play one item | `POST /Items/<id>/PlaybackInfo` with a JSON body |
| Stop a transcode | `DELETE /Videos/ActiveEncodings?DeviceId=surfcult&PlaySessionId=<session>` |

Paging follows `TotalRecordCount`. If a list ends before its count, the sync fails rather than caching a partial library. An item that appears on two pages is cached once.

Songs are asked for only from libraries whose `CollectionType` is `music`, one library at a time, and each page is parsed into cache entries before the next is read, so at most one page (500 songs) is held as JSON. The other lists are read whole and then parsed. Songs come sorted by album artist, album, disc, track and name, so one album's songs are together in the cache and a `sequential` music channel plays albums in order.

surfcult never calls a `/Sessions/Playing` endpoint or anything else that reports playback, so the owner's watched and continue-watching lists stay as they are. A test checks every request path.

## Playback URLs

`client.playback(&id, start_ms)` asks Jellyfin how the Pi should play an item, starting `start_ms` into it, and returns a `Playback`. Jellyfin makes the decision by comparing the item with the device profile surfcult sends. The request also carries `StartTimeTicks` (milliseconds times 10,000), a 200 Mbps bitrate ceiling so bitrate never rules out direct play on the LAN, `SubtitleStreamIndex: -1` so subtitles never force a transcode, and `EnableDirectStream: false`. Jellyfin's first media source is used.

The request names a user (`UserId`). An API key has no user of its own, and Jellyfin 12.4.0 answers `400 Error processing request.` to a request that carries a device profile but no user; the Pi check of 2026-11-24 found this. `JellyfinConfig::user_id` (from `SURFCULT_JELLYFIN_USER_ID`, see `JellyfinConfig::user_id_from_env`) sets the user. Without it the client asks `GET /Users` once and keeps the first user that is not disabled. The profile lists every subtitle format as `External`. surfcult never shows subtitles, but on Jellyfin 12.4.0 an empty `SubtitleProfiles` makes any item with a subtitle stream transcode, however well its video and audio fit, and `Drop` and `Embed` were refused too (found on the Pi check of 2026-11-24). The player turns subtitles off for programs (`sid=no`). Playback is asked for as that user but never reported, so only that user's permissions apply, not their watched list.

- `Playback::Direct { path_and_query, seek_ms }` when Jellyfin reports `SupportsDirectPlay`: `/Videos/<id>/stream?static=true&mediaSourceId=<source>`, the file as stored, or `/Audio/<id>/stream?...` when the media source has no `Video` stream (a song; its cover art is an `EmbeddedImage` stream). The stream starts at the beginning, so the player seeks to `seek_ms`.
- `Playback::Transcode { path_and_query, session }` otherwise: Jellyfin's `TranscodingUrl`, a progressive MPEG-TS stream `/videos/<id>/stream.ts?...` that already starts at the position, so the player must not seek. surfcult removes every `api_key` or `ApiKey` parameter Jellyfin puts in it, in any case, and sets `DeviceId=surfcult`, `PlaySessionId` (from the response), `VideoCodec=h264`, `h264-videobitdepth=8` (so a 10-bit H.264 source is re-encoded, never stream-copied to the Pi), `AudioCodec=aac`, `VideoBitrate=8000000`, `MaxWidth=1920`, `MaxHeight=1080` and `StartTimeTicks`, replacing whatever Jellyfin wrote. Other parameters, such as `MediaSourceId` and `AudioStreamIndex`, are kept. A transcode URL that is not `stream.ts` (an HLS `master.m3u8`, for example) is refused: HLS took about 19 s to start at an offset. A transcode URL for an item other than the one asked for is `JellyfinError::WrongItem`; ids match with or without dashes and in any case.

`Playback::url(base_url)` joins the path to the server address. The URLs carry no key; the player must send the `Authorization` header itself (`run::JellyfinResolver` gives it to MPV as the `http-header-fields` property). Jellyfin's `ErrorCode` (such as `NoCompatibleStream`), a response with no media source, or one that offers neither direct play nor a transcode is `JellyfinError::NoPlayback`.

When playback ends or the channel changes, `client.stop_transcode(&session)` sends the `DELETE` above, and Jellyfin stops encoding. A 404 means Jellyfin no longer has that session, so it counts as stopped. `Playback::transcode()` returns the session, or `None` for direct play. `surfcult run` stops each transcode when its program leaves the screen ([run.md](run.md#transcodes)).

### The device profile

The rules live in one file, `src/library/profile.rs`, which follows ADR 0001 decisions 3 and 4. The tuner's `needs_transcode` and the `DeviceProfile` sent to Jellyfin both read its constants, so changing the ADR means editing that file only.

- One direct-play entry with video codecs `hevc,h264` and audio codecs `aac,mp3,flac,opus,ac3,eac3,dts,truehd`, and an `Audio` entry with the same audio codecs, so songs play directly. Neither names a container, so any container plays directly. There is no audio transcoding profile: a song in another codec (ALAC, WMA) cannot play, and the TV treats it as an unplayable program. The owner's library is FLAC and MP3.
- A codec profile for each video codec: bit depth at most 10 for `hevc` and 8 for `h264`, height at most 1080, and `VideoRangeType` `SDR` only, so HDR10, HLG and Dolby Vision are transcoded. The conditions are not required, so a stream with an unknown bit depth still plays directly, the way `needs_transcode` reads a missing depth as 8 bit.
- One transcoding profile: container `ts`, H.264 and AAC, protocol `http` (progressive, not HLS).

`needs_transcode` cannot see HDR, because the cache stores no video range; Jellyfin can, so an HDR item may get a transcode the tuner did not plan a long clip for.

## The API key

Jellyfin 12.4.0 accepts the key only in the header `Authorization: MediaBrowser Token="KEY"`. `X-Emby-Token` and an `api_key` query parameter return 401, so the client uses the header and nothing else. The key is never put in a URL or a command line.

`ApiKey::from_env()` reads the key from the first of these that is set and not empty:

1. `SURFCULT_JELLYFIN_API_KEY`, the key itself.
2. `SURFCULT_JELLYFIN_API_KEY_FILE`, the path of a file that holds the key. Whitespace around the key is trimmed. Prefer this form: the key stays out of the process environment listing, and the file can have mode 600.

`ApiKey` has no `Display`, and its `Debug` output is `ApiKey(<redacted>)`. Errors never contain it; if a transport's error text repeats it, the client replaces it with `[redacted]`. A key with a control character, a double quote or a backslash is refused because it would break the header.

The server's address is passed in as `JellyfinConfig { base_url, key }`. It is not in `channels.toml` yet. The HTTP client does not follow redirects, so the key cannot go to another host; a redirect is an error, and `base_url` must be the address Jellyfin answers on. Connecting takes at most 5 seconds and a whole request 30 seconds. Both `http` and `https` work; `https` uses rustls with built-in root certificates and needs no system library.

The requests carry no user id, so the key decides what is visible. Give surfcult a key that can read the libraries it schedules and cannot change anything.

## What is cached

For each kept item: id (the Jellyfin item id), title, kind (`movie`, `episode`, `music_video` or `song`), runtime, file path, chapters, video streams (codec, width, height, bit depth), audio streams (codec, channels, language), the names of the libraries it is in, its tags and genres, and the time of the sync. An episode also keeps its series id (`SeriesId`), series name (`SeriesName`), season (`ParentIndexNumber`) and episode number (`IndexNumber`). A music video or song also keeps its artists (`Artists`, in Jellyfin's order, empty names dropped) and its album (`Album`); a movie or episode keeps neither. A song also keeps its album id (`AlbumId`, `MediaItem::album_id`): Jellyfin serves the album's cover at `/Items/<album id>/Images/Primary`.

- Jellyfin counts time in ticks of 100 ns. Runtime and chapter starts are stored in milliseconds, ticks divided by 10,000.
- Items come back in the order Jellyfin listed them (sorted by name). Chapters and streams keep their order.
- The file path is in the cache and in `Store::library_items()`, but not in `MediaItem`, which is unchanged. The path is for reconciling items by path when an id changes (see below).
- An episode carries its series' tags and genres after its own, without repeats, because Jellyfin usually sets them on the series. Empty names are dropped.
- Libraries are Jellyfin's media folders. An item gets the name of each library whose recursive listing includes it. The library list needs a key that may read `/Library/MediaFolders`.
- Collections are Jellyfin `BoxSet` items. Each keeps its members in Jellyfin's order. A movie or episode member is kept by its item id, and a series member by its series id when at least one of its episodes is cached. Other members (an item that was skipped, a series with no cached episode) are left out.
- Music videos are Jellyfin `MusicVideo` items, such as those in a library of type `musicvideos`. Each is its own kind, `MediaKind::MusicVideo`, so code that treats movies and episodes differently can tell it apart. It is played like a movie: the same device profile, playback request and stream resolver. The banner, the guide screen, `surfcult now` and the read API title it `Artist – Title`, several artists joined with commas, or just the title when it has no artists (`MediaItem::display_title`).
- Songs are Jellyfin `Audio` items in a library of type `music`, `MediaKind::Song`. A song gets that library's name and no video streams, even when its file carries cover art. Songs share genres with videos ("Comedy", "Rock"), so only a channel with a `picture` plays them, and such a channel plays nothing else; see [Channel sources](#channel-sources). The banner and guide title a song `Artist – Title`, as for a music video. In a scheduled week a song is a program like any other: breaks cut it at multiples of `breaks.every` (most songs are shorter than that), `between` plays between songs, and the next song is queued as the next program is today.
- A database from before migration 0004 has no libraries, tags, genres or series until the next sync replaces the cache.
- Playback URLs are not cached; `playback` asks Jellyfin each time.

## Keeping the schedule in step

Tdarr replaces files in place, so a sync can find an item with a new runtime, a new id or no item at all. `Store::replace_library` compares the cache it replaces with the new items (`library::reconcile`, which does no I/O) and returns `LibraryChanges`; the sync puts it in `SyncReport::changes`.

- Each cached item is matched with a new one by id, or else by file path with a new item whose id the cache did not have. An item matched by path is `renamed` (old id, new id).
- A matched item with a different runtime is `resized`, under its new id.
- A cached item matched neither way is `vanished`. It is not kept in the cache.
- New items are not listed; nothing is scheduled with them yet.

In the same transaction every stored event of a renamed item, on every channel and at any time, takes the new id. Rewriting the events, rather than keeping the old id as an alias, keeps frozen and future events playable without any lookup at play time: Jellyfin only knows the new id. A sequential channel also continues after the renamed item, because it reads its place from the events. `surfcult run` then follows the changes in each channel's schedule ([Schedule generator](schedule.md#following-library-changes)).

Whether Jellyfin keeps an item's id when Tdarr replaces its file is not verified; the path match covers the case where it does not.

## Channel sources

`library::resolve_pool(channel, plays, source, media)` turns a channel's `source` into its program pool from the cache. Names match exactly, including case. `plays` is `channel.plays()`: `Plays::Songs` for a channel with a `picture` (`picture = "cover"` or `"visualizer"`), `Plays::Videos` for every other. A song channel's sources match only songs, and a video channel's never match a song, so a video channel with `jellyfin_genre = "Comedy"` does not pick up comedy songs. The cache answers each source with one SQL query on the item kind and the label (`Store::library_items_matching`, through `MediaSource::query_items`), so a video channel never reads the songs and a genre never reads the whole cache; migration 0008 indexes the labels by name for it.

| Source | Pool |
|---|---|
| `jellyfin_collection` | The first collection with that name, members in collection order. A series member becomes its episodes in episode order; movies, episodes and music videos stay as they are. An empty collection gives an empty pool. |
| `jellyfin_library`, `jellyfin_tag`, `jellyfin_genre` | Every item in that library or with that tag or genre, in cache order (Jellyfin's sort by name; songs by album), except that one series' episodes are gathered, in episode order, where its first matching episode was. A library of music videos (`jellyfin_library = "Music Videos"`) gives its music videos; a genre or tag also matches music videos that carry it. |
| `jellyfin_series` | The episodes of the first series in cache order with that name, in episode order. |

Episode order is season, then episode number. Season 0 (Jellyfin's specials) comes first, a missing number sorts after the numbered ones, and ties keep cache order. An item reached twice, such as an episode a collection lists both directly and through its series, is kept once at its first place.

A collection that does not exist, and a library, tag, genre or series that matches no cached item, returns a `PoolError` whose message names the channel and the source, for example `channel 3: jellyfin_tag "Retro" matches nothing in the library`, or `matches no song in the library` on a song channel. A cache read failure is a `PoolError` too. No source falls back to the whole library.

## What is skipped

A sync skips, and counts in `SyncReport::skipped`:

- Videos with no video stream. Jellyfin lists a missing episode as an item with no streams; it cannot be played. Songs need no stream.
- Items with no runtime.
- Items whose JSON does not have the expected shape (for example a number sent as text).

Unknown fields never matter. Apart from an item's `Id`, every field may be missing or `null`: a video stream with no codec is cached as codec `unknown`, one with no size as 0 by 0, and an item with no name gets its id as the title.

## Sync cost

The owner's music library has 31,785 songs. `sync_cost_of_a_music_library` in `src/jellyfin/tests.rs` (ignored; run it with `cargo test --release -- --ignored --nocapture sync_cost`) syncs 32,000 generated songs, each in the JSON shape Jellyfin sends with two genres, through the fake transport into a file-backed store. Measured on Kvant (Ryzen 9 9900X) on 2026-11-28, release build:

| Step | Time |
|---|---|
| Fetch and parse 64 pages (the fake builds the JSON too) | 0.47 s |
| Write the cache, one transaction | 0.35 s |
| Fetch and replace again, with reconciliation | 1.0 s |
| One genre's pool, 7,432 songs with their streams and labels | 38 ms |
| One genre's pool on a video channel | 0.2 ms |
| The whole cache (`list_items`) | 64 ms |

The database grew to 25 MB and the test process peaked at 67 MB resident, fake server included. The Pi 4B is several times slower and writes to an SD card, so expect a write to take seconds there; that has not been measured. Two things keep it off the TV's loop most of the time: an unchanged library is not written again (see [How it works](#how-it-works)), and `surfcult run` keeps each channel's resolved pools until a sync writes a changed library ([run.md](run.md#schedules-and-the-clock)). A sync that does change the library still writes the whole cache on the loop's thread.

## Tests

No test needs a Jellyfin server. The fixtures in `tests/fixtures/jellyfin/` are real Jellyfin 12.4.0 responses with fake names, paths and ids. The `Tags`, `Genres` and `SeriesId` fields in `items-page.json`, the series member of Fixture Collection B, and the files `series-page.json`, `media-folders.json` and `library-children-by-parent-id.json` were written by hand in the same shape; they were not recorded from a server. So was `music-videos-page.json`, two `MusicVideo` items (one with two artists and an album, one with neither) that the music video tests add to the items page, with a `Music Videos` library holding them, and `songs-page.json`, three `Audio` items (a FLAC song with an album, an album id and cover art, an MP3 single with two artists and no album, and one without a runtime) served from a `Music` library. Tests give the client a fake transport (the `JellyfinHttp` trait: `get`, `post_json` and `delete`) that serves them. The two `playback-info-*.json` fixtures are made up in the shape of Jellyfin's `PlaybackInfoResponse`, and the transcode one puts a key in `TranscodingUrl` as Jellyfin does. For playback tests the fake applies the device profile the client sent to the item's streams and answers with the direct-play or the transcode fixture, rewritten to name the requested item; a census test checks that profile against every case in ADR 0001 decision 3 and against `needs_transcode`. A few tests start a listener on `127.0.0.1` to check the real `ureq` transport (header sent, POST body, DELETE, 401, redirect not followed, closed port); they do not leave the machine.

Not covered by tests: whether the URLs play in MPV on the owner's network. That needs a manual check on the Pi.
