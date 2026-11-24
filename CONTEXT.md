# Context

The words surfcult's code and documentation use, and the code item each one names.

## Channels and media

- **Channel**: a numbered entry in `channels.toml`, `config::Channel`. It has a name, a plan (one source in one mode, or time-of-day blocks) and optional grid, break and filler settings.
- **Source**: where a channel's items come from in Jellyfin: a library, collection, tag, genre or series, `config::Source`.
- **Media source**: the interface that lists and resolves media items without saying where they come from, `library::MediaSource`. Its two adapters are the library cache in the `Store` and `library::FakeMediaSource` for tests.
- **Library cache**: the `library_*` tables in the store, a copy of the Jellyfin library. Everything except the sync step reads the library from here, so it keeps working while Jellyfin is down.
- **Sync**: `jellyfin::sync`, which fetches the library from Jellyfin and replaces the library cache in one transaction. The caller passes the sync time; a failed sync leaves the cache as it was.
- **Sync worker**: `jellyfin::SyncWorker`, which fetches the library on its own thread at a fixed interval and sends each result back. The store's owner writes the newest snapshot with `SyncWorker::apply`; the worker never touches the store.
- **Program pool**: the ordered items a channel draws its programs from, built from its source by `library::resolve_pool`. Until #30 only collections resolve; every other source returns `PoolError::Unsupported` instead of the whole library.

## Clips

- **Clip**: a short local video file and its probed duration, `clips::Clip`: an ad, ident, bumper, tune-in clip, filler or slate.
- **Clip pool**: a named directory of clips on the Pi, `clips::Pool`, scanned by `clips::ClipLibrary`. It is not a program pool.
- **Tune-in clip**: the clip the `tuner` plays between static and the program when the viewer changes channel. It is picked from the tune-in clip pool to cover the program's expected start-up time.

## Schedule

- **Event**: one scheduled stretch of one media item, a half-open interval `[start_ms, end_ms)`, `timeline::Event`. At a boundary the later event is the one on.
- **Program**, **break**, **filler** and **ident**: the four `timeline::EventKind`s. A program is an item from the program pool. A break is a stretch of clips, such as ads, from a channel's break pool; filler fills a gap up to the next grid slot; an ident is a short station identification. Only program events are generated so far.
- **Channel schedule**: a channel's stored events. `schedule::extend` keeps it filled to the horizon and `Store::events_between` reads it.
- **Horizon**: how far ahead of now a schedule is kept filled, seven days (`schedule::HORIZON_MS`).
- **Extend**: one `schedule::extend` call. It starts at the later of now and the end of the last stored event, generates up to the horizon, and writes the new events in one transaction.
- **Trusted clock**: the caller's statement, through the `clock_trusted` argument, that the system clock is synchronized. `extend` does nothing without it, so a Pi with a wrong clock never stores a wrong week.
- **Sequential**: a mode, `config::Mode::Sequential`. Items play in program pool order, continuing after the resume point and wrapping from the last item to the first.
- **Shuffle** and **cooldown**: in shuffle mode (`config::Mode::Shuffle`) each program slot picks at random, seeded from the channel and the slot's start, from the items not cooling down. With N playable items and k = max(1, ceil(N / 4)), the N - k most recently played distinct items are cooling down.
- **Resume point** (cursor): the media id of the channel's last stored program event, which says where the next extend continues. It has no table of its own; `extend` derives it from the events, so it cannot disagree with them.
- **Recent history**: the media ids of the channel's most recent stored program events before the new window, newest first, read with `Store::recent_programs_before` and passed as `Resume::recent`. Only shuffle reads more than one row, to find the items cooling down; a sequential channel reads just its last program (`Store::last_program_before`). It is not the `playback_history` table, which records what the tuner started.

## Running

- **Store**: the one SQLite file, `store::Store`. It holds the channel schedules, the playback history and the library cache. The composition root owns it and passes it to each call (`&mut` to write, `&` to read); nothing else keeps a store between calls.
- **Composition root**: the code in the binary that creates the store, the player and the tuner and drives them on the Pi. It is not written yet (#23).
- **Player**: the screen as the tuner sees it, `player::Player`: static, a clip, a queued program, seek, stop and overlays. `player::MpvPlayer` drives a real MPV over its JSON IPC socket; `player::FakePlayer` records commands for tests.
- **Stream resolver**: what turns a program's media id and start position into a URL, its HTTP headers and the media position the stream begins at, `player::mpv::StreamResolver`. It keeps Jellyfin out of the player.
- **Overlay**: a premultiplied BGRA bitmap drawn over the video in one of 64 layers, `player::Overlay`.
- **Watchdog**: the part of `MpvPlayer::poll_event` that notices a dead or hung MPV (a hung one fails a `get_property pid` check sent at most every few seconds), starts a new one and reports `PlayerEvent::Restarted`. The tuner then tunes the current channel again.
- **Look-ahead**: the tuner looks up the schedule at the moment the tune-in clip will end, not at the key press, with `timeline::program_from`, so the program is queued at the right offset. If a break is on then, it gets the program after the break at that event's `media_offset_ms`.
- **Live position**: where the schedule says the media should be at a given moment, the `offset_ms` of the event `timeline::lookup` finds.
- **Late start**: a program that starts more than `Timing::late_tolerance_ms` (500 ms) behind its live position. The tuner seeks forward to the live position once.
