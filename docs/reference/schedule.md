# Schedule generator

The code is in `src/schedule/`. It turns a pool of media items into `timeline::Event`s for one channel: `sequential` and `shuffle` channels, programs back to back, seven days ahead. Breaks, grids, `weighted`, `blocks` and regeneration are not done yet.

## Two layers

`generate(&Params, pool, &Resume, start_ms, end_ms)` is a pure function inside the crate. It reads no clock and does no I/O. `Params` holds the channel number and the `Mode`. `Resume` holds `recent`, the media ids of earlier programs, newest first; the first of them is the cursor, the last item scheduled. It returns `Generated { events, cursor, notes }`. Times are Unix epoch milliseconds.

`schedule::extend(&mut Store, &Channel, pool, now_ms, clock_trusted)` keeps a channel's week filled. The caller supplies the pool as a slice of `MediaItem`s: it takes `source` from the channel's `Plan::Fixed`, resolves it with `library::resolve_pool` and passes the result to `extend`. Until #30 only a `jellyfin_collection` source resolves, to the collection's cached members in collection order; the other sources return `PoolError::Unsupported`, so a channel never falls back to the whole library. `extend` returns `Extended::ClockNotTrusted` and touches nothing when `clock_trusted` is false, because a Pi with an unsynchronized clock must not store a wrong week. Otherwise it starts at the later of `now_ms` and the end of the last stored event and generates up to `now_ms` plus seven days (`HORIZON_MS`). If the schedule already reaches that far it returns `Extended::UpToDate`. It never reads the system clock.

The resume point comes from the stored events, not from a separate cursor. The cursor is the media id of the channel's last program event that starts before the new window, at any time, past or future. A sequential channel reads only that one event (`Store::last_program_before`). A shuffle channel reads its most recent program events, newest first, in pages of twice the pool's length (`Store::recent_programs_before`, which walks back on the primary key). It reads the next page, bounded by the oldest start time so far, until the rows hold the N - k distinct pool items cooling down (see Shuffle) or the history ends, and passes the rows as `Resume::recent`. One page is usually enough; the read reaches further back only when the recent programs are mostly ids no longer in the pool, or when the history is shorter than N - k distinct pool items, in which case it reads the whole history. Writing the new events is the only write and is one transaction, so a crash cannot leave the schedule and the resume point disagreeing. Because the resume point is read from the events, `Store::replace_events_from` also moves it back to the last program it keeps.

## Rules for every run

- Each program is one `Program` event with `media_offset_ms` 0 and `media_duration_ms` set to the item's runtime. It starts where the last one ended.
- The events cover the whole window with no gap and no overlap. The last event may end after the window.
- An item with a runtime of 0 is skipped. An id that appears twice in the pool is used once.
- A pool with nothing playable gives no events and `Note::NoPlayableItems`. Nothing is written, so the resume point stays as it was.
- A `grid`, `breaks` or `filler` setting on the channel is ignored for now.

## Sequential

Items play in the pool's order. A run continues after the cursor item and wraps from the last item to the first. With no cursor it starts at the first item. If the cursor item is no longer in the pool, the run starts at the first item and reports `Note::CursorNotInPool`.

## Shuffle

Each program slot picks at random from the items that are not cooling down.

- Let N be the number of distinct playable items in the pool and k = max(1, ceil(N / 4)). Walk the earlier programs newest first, the resumed history and then this run's picks, collecting distinct pool ids until there are N - k of them. Those items are cooling down; every other playable item is eligible.
- An item that never played, or was added to the pool since, is eligible. An id that is no longer in the pool is ignored and does not count toward N - k. When the history holds fewer than N - k distinct pool items, fewer items are cooling down.
- The pick draws an index into the eligible items, in pool order, with SplitMix64 (`src/schedule/rng.rs`) seeded from the channel number and the slot's `start_ms`. The seed has no date, counter or run state, and the result does not depend on the `rand` crate, a hasher or the Rust version.

What this gives:

- No item repeats within N - k + 1 consecutive programs. With N = 1 the one item always plays. With N >= 2 the item that just played never plays next. With N up to 4, k is 1, so once the history is full the items rotate in a fixed order.
- An eligible item stays eligible until it plays, so every item plays eventually.
- A run that stops at an event boundary and resumes from the stored events produces exactly what an unbroken run would. This holds because `extend` reads back until it has N - k distinct pool items or reaches the first stored program, so a resumed run sees the same cooling items as the unbroken one. A caller of `generate` that passes a shorter history loses that guarantee.

Limits:

- There are no passes, so there is no guarantee that every item plays once before any item plays twice.
- If the whole stored history holds fewer than N - k distinct pool items, for example on a new channel or after the pool changed, fewer items are cooling down and more are eligible.

## Notes for later work

- Breaks will cut a program into several events with the same `media_id`. The cooldown walk already counts them as one item, but each event uses a row of the history read, so the read may need more pages.
- A run stops with `ScheduleError::TooManyEvents` after 200,000 events, which only happens when runtimes are seconds long.
