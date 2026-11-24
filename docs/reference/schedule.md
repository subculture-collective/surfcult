# Schedule generator

The code is in `src/schedule/`. It turns a pool of media items into `timeline::Event`s for one channel: `sequential` and `shuffle` channels, programs back to back, seven days ahead. Breaks, grids, `weighted`, `blocks` and regeneration are not done yet.

## Two layers

`generate(&Params, pool, &Resume, start_ms, end_ms)` is a pure function inside the crate. It reads no clock and does no I/O. `Params` holds the channel number, the `Mode` and the time zone. `Resume` holds the cursor (the media id of the last item scheduled) and `played`, the media ids of earlier programs, oldest first. It returns `Generated { events, cursor, notes }`. Times are Unix epoch milliseconds.

`schedule::extend(&mut Store, &Channel, pool, tz, now_ms, clock_trusted)` keeps a channel's week filled. The caller supplies the pool as a slice of `MediaItem`s: it takes `source` from the channel's `Plan::Fixed`, resolves it with `library::resolve_pool` and passes the result to `extend`. Until #30 only a `jellyfin_collection` source resolves, to the collection's cached members in collection order; the other sources return `PoolError::Unsupported`, so a channel never falls back to the whole library. `extend` returns `Refill::ClockNotTrusted` and touches nothing when `clock_trusted` is false, because a Pi with an unsynchronized clock must not store a wrong week. Otherwise it starts at the later of `now_ms` and the end of the last stored event and generates up to `now_ms` plus seven days (`HORIZON_MS`). If the schedule already reaches that far it returns `Refill::UpToDate`. It never reads the system clock.

The resume point comes from the stored events, not from a separate cursor. `extend` reads the channel's program events that start before the new window (`Store::programs_before`), at any time, past or future, and counts consecutive events of one media id as one program. The last of them is the cursor. Shuffle also passes all of them as `Resume::played`. Writing the new events is the only write and is one transaction, so a crash cannot leave the schedule and the resume point disagreeing. Because the resume point is read from the events, `Store::replace_events_from` also moves it back to the last program it keeps.

## Rules for every run

- Each program is one `Program` event with `media_offset_ms` 0 and `media_duration_ms` set to the item's runtime. It starts where the last one ended.
- The events cover the whole window with no gap and no overlap. The last event may end after the window.
- An item with a runtime of 0 is skipped. An id that appears twice in the pool is used once.
- A pool with nothing playable gives no events and `Note::NoPlayableItems`. Nothing is written, so the resume point stays as it was.
- A `grid`, `breaks` or `filler` setting on the channel is ignored for now.

## Sequential

Items play in the pool's order. A run continues after the cursor item and wraps from the last item to the first. With no cursor it starts at the first item. If the cursor item is no longer in the pool, the run starts at the first item and reports `Note::CursorNotInPool`.

## Shuffle

No item repeats until every other item has played. One trip through the pool is a pass.

- A pass is a Fisher-Yates shuffle driven by SplitMix64, both written in `src/schedule/rng.rs` so the result does not depend on the `rand` crate, a hasher or the Rust version.
- The seed is the channel number, the local date in the channel's time zone when the pass is drawn, and the number of passes already drawn in this run. The date is a calendar date, so a 23-hour or 25-hour day does not change it.
- The cursor is one media id and cannot say which items the current pass has used. `extend` therefore passes the ids of the channel's earlier program events as `Resume::played`. The generator replays them to find the items already used in the current pass, plays the rest of that pass in the seeded order, then starts a new one. A restart does not replay an item inside the same pass. A pass never opens with the item that just played, unless the pool has one item.

Limits:

- A run resumed on the same local date plays the rest of the pass exactly as an unbroken run would. Later passes, or a different date, can differ from an unbroken run but are still valid shuffles.
- If the pool changes, a new item joins the current pass and a removed one is forgotten.
- Reading the whole event history is what makes the replay exact. If old events are deleted, the replay may start in the middle of a pass and misjudge it once.

## Notes for later work

- Breaks will cut a program into several events with the same `media_id`. The shuffle replay already counts consecutive events of one media id as one program.
- A run stops with `ScheduleError::TooManyEvents` after 200,000 events, which only happens when runtimes are seconds long.
