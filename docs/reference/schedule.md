# Schedule generator

The code is in `src/schedule/`. It turns pools of media items into `timeline::Event`s for one channel: `sequential`, `shuffle` and `weighted` channels and `blocks` channels, programs back to back with commercial breaks, seven days ahead, and regenerates the future after a change. Grids are not done yet.

## Two layers

`generate(&Params, pool, &Resume, start_ms, end_ms)` is a pure function inside the crate. It reads no clock and does no I/O. `Params` holds the channel number, the `Mode`, the `weights` (used by `weighted`) and an optional `BreakPlan` (the break interval and length, the break pool's clips and the filler pool's clips). `Resume` holds `recent`, the media ids of earlier programs, newest first, of which the first is the cursor, the last item scheduled; and `last_break`, the start time and media id of the last break clip. It returns `Generated { events, cursor, notes }`. Times are Unix epoch milliseconds.

`generate_blocks(&Layout, &Resume, start_ms, end_ms)` is the same for a blocks channel. `Layout` holds the channel number, the `BreakPlan`, one `Lane` (mode, weights and pool) per block in config order and then one for the default if there is one, the default's index, the spans from `blocks::spans(blocks, timezone, start_ms, end_ms)` and the channel's filler clips. `spans` is pure too: it turns the blocks' local times into epoch instants.

`schedule::extend(&mut Store, &Channel, pools, clips, timezone, now_ms, clock_trusted)` keeps a channel's week filled. `clips` is the scanned clip pools, `ClipLibrary::pools()`; a channel finds its break pool and its `filler` pool there by name, and a missing pool counts as empty. The caller supplies the program pools as `(Source, Vec<MediaItem>)` pairs: it resolves each source of `channel.plan.sources()` with `library::resolve_pool(channel.number, source, &store)` and passes the pairs to `extend`. A source with no pair counts as an empty pool. `timezone` is the config's `timezone`; only blocks channels read it. Every source kind resolves; a source that matches nothing is an error naming the channel and the source, so a channel never falls back to the whole library. The order each source gives is in [Jellyfin source](jellyfin.md#channel-sources). `extend` returns `Extended::ClockNotTrusted` and touches nothing when `clock_trusted` is false, because a Pi with an unsynchronized clock must not store a wrong week. Otherwise it starts at the later of `now_ms` and the end of the last stored event and generates up to `now_ms` plus seven days (`HORIZON_MS`). If the schedule already reaches that far it returns `Extended::UpToDate`. It never reads the system clock.

The resume point comes from the stored events, not from a separate cursor. The cursor is the media id of the channel's last program event that starts before the new window, at any time, past or future. A sequential or weighted channel reads only that one event (`Store::last_program_before`); weighted does not use it to pick. A shuffle channel reads its most recent program events, newest first, in pages of twice the pool's length (`Store::recent_programs_before`, which walks back on the primary key). It reads the next page, bounded by the oldest start time so far, until the rows hold the N - k distinct pool items cooling down (see Shuffle) or the history ends, and passes the rows as `Resume::recent`. One page is usually enough; the read reaches further back only when the recent programs are mostly ids no longer in the pool, or when the history is shorter than N - k distinct pool items, in which case it reads the whole history. Writing the new events is the only write and is one transaction, so a crash cannot leave the schedule and the resume point disagreeing. Because the resume point is read from the events, `Store::replace_events_from` also moves it back to the last program it keeps. A channel with breaks also reads its last stored break event from the last 24 hours (`Store::last_break_before`) as `Resume::last_break`, so the break rotation continues where it stopped. The read stops at that bound, so a channel that never stores a break does not walk its whole history on every extend.

## Rules for every run

- Without breaks, each program is one `Program` event with `media_offset_ms` 0 and `media_duration_ms` set to the item's runtime. It starts where the last one ended.
- The events cover the whole window with no gap and no overlap. The last program may end after the window. A program cut by breaks is written whole, so a run always ends on a program's last segment.
- An item with a runtime of 0 is skipped. An id that appears twice in the pool is used once.
- A pool with nothing playable gives no events and `Note::NoPlayableItems`. Nothing is written, so the resume point stays as it was.
- A `grid` setting on the channel is ignored for now.

A blocks channel differs in two ways: a program still on at the end of its block is cut there, and time with nothing to play has no events. See Blocks.

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

## Weighted

Each program slot picks one playable item, with probability proportional to its weight.

- `weights = { "Toy Story" = 3, "Cars" = 2 }` on a `weighted` channel, block or default maps an item title to a whole number of at least 1. Every pool item with that exact title has that weight; every other item weighs 1. `weights` with any other mode is a config error.
- The pick draws a number below the total weight with SplitMix64 seeded from the channel number and the slot's `start_ms`, as the shuffle does, and takes the first item in pool order whose running total of weights is above it.
- Over many picks each item's share approaches its weight divided by the total. The same inputs give the same picks on every machine.
- It reads no history, so a resumed run matches an unbroken one.

Limits:

- There is no cooldown. An item can play twice in a row, and a heavy item often will.
- A weight whose title matches no playable item does nothing and is reported as `Note::WeightNotInPool { title }`. Titles are matched exactly, so a typo shows up there.

## Blocks

A `blocks` channel lists `[[channel.block]]` entries, each with `days`, `start`, `end`, a `source`, a `mode` and, for `weighted`, `weights`. It may also have a `default = { source, mode, weights }`, which plays the time no block covers.

### Where a block falls

- Block times are local wall-clock times in the config's `timezone`. Each block covers `[start, end)` on each of its days; `days` are the days it starts on, every day when left out.
- The spans hold real instants, so a block from 00:00 to 24:00 lasts 23 hours on the day the clocks go forward and 25 on the day they go back. A block from 06:00 to 12:00 in Chicago starts at 11:00 UTC in summer and 12:00 UTC in winter, including on both change days.
- A local time that happens twice, when the clocks go back, means its first occurrence. A local time that does not happen, when the clocks go forward, means the moment they jump: in Chicago on the spring change, 02:30 means 03:00 CDT. A block wholly inside the skipped hour does not play that day.

### What plays

- A block plays its own pool in its own mode. Its first program starts at the block's start.
- A program still on at the block's end is cut there: the event ends at the boundary and keeps the item's runtime as `media_duration_ms`. A break or clip on at that moment is cut the same way, and break clips after it are dropped. Breaks that would start at or after the block's end are not filled and do not count toward `Note::ShortBreaks`. The break rotation continues after the last break clip that was kept.
- Time no block covers plays the `default` when there is one. Otherwise it plays the channel's `filler` pool: its clips in pool order, over and over, counted from the start of the uncovered stretch, with the last clip cut at the stretch's end. Because the cycle is counted from the stretch's start, a run that resumes in the middle of it plays the same clips.
- Such a filler stretch can last hours, unlike a break. The tuner cannot play filler clips yet, and it does not queue the next block's first program early: when filler is on as the tune-in clip ends and the next program starts more than 10 minutes later, it shows static with `Reason::NoProgramSoon` ([tuner.md](tuner.md), "A break when the clip ends").
- With no default and no filler clip, uncovered time has no events. The tuner treats that time as a gap ([tuner.md](tuner.md), "When there is nothing to show"). `Note::DeadAir { ms }` says how much there was.
- A block whose pool has nothing playable falls back the same way, and `Note::NoPlayableItems` is reported.

### Keeping each block's place

- Each block, and the default, keeps its own place. A sequential block continues after the newest program, anywhere in the channel's history, that is in its pool, so Saturday's cartoons continue from last Saturday's. A shuffle block's cooldown counts only programs from its pool. A weighted block needs no history.
- A program counts for every block whose pool holds it, so two blocks that share items share their place.
- `extend` reads the channel's programs, newest first, in pages of twice the pools' total length, until each sequential block has one program from its pool and each shuffle block the N - k distinct items it keeps cooling down, or the history ends. A block that has never played, such as a new one, makes it read the whole history.
- A run that resumes at an event boundary gives what an unbroken run would, across days and clock changes.

## Breaks

A channel with `breaks = { every, length, pool }` has its programs cut, and each cut filled with a break. Breaks add to the slot: a 22-minute program with two 2-minute breaks occupies 26 minutes, as three program events and the break events between them.

### Where a program is cut

- The nominal cuts are the multiples of `every` in the program's own time, strictly inside its runtime. There is no cut at the end; the break between programs is grid work.
- Each cut moves to the nearest Jellyfin chapter mark within 60 seconds (`schedule::SNAP_MS`), the earlier mark on a tie, as long as the mark is after the previous cut and before the end. With no such mark the cut stays at the interval.
- Every program event of a cut item has the item's `media_id` and runtime as `media_duration_ms`. The first starts at offset 0 and each later one at the previous cut, so a tune-in seek is always "this media at this offset".

### What fills a break

- The break pool's clips are walked once, in pool order, starting after the last clip placed. Each clip that fits in the time left is placed as one `Break` event as long as the clip, rounded up to a whole millisecond. A clip is never cut and never plays twice in one break. A clip longer than the time left is skipped and waits for a later break.
- The time left over is one `Filler` event from the channel's `filler` pool, exactly as long as the shortfall. It plays the shortest filler clip at least that long, or the longest filler clip if none is long enough. This pad is the only piece that may stop before its clip ends, or run past it, so the filler pool should hold idents and slates.
- Without a usable filler clip a break comes out shorter than `length`. A break with nothing to show at all is left out and the program runs on uncut. Both count toward `Note::ShortBreaks { count }`.

### Rotation and frequency

`channels.toml` has no rotation or frequency settings, so the generator applies fixed rules. Clips rotate in pool order from break to break, and a clip plays at most once per break. The rotation continues after the channel's last stored break clip; if that clip is no longer in the pool it starts again at the first clip. The rotation remembers its place for 24 hours (`ROTATION_MEMORY_MS`): a break that starts more than 24 hours after the last break clip started begins at the first clip. An unbroken run applies the same rule, so `extend` only needs to read the last day of events, and a resumed run still matches an unbroken one. Clips that share a media id, such as one file reached through two symlinks, count once, at the first one's place in the pool.

### Clip ids

A clip's `media_id` is its canonical path as text (`schedule::clip_media_id`). Clips whose path is not valid UTF-8 are left out. The event kind tells a clip (`Break`, `Filler`) from a Jellyfin item (`Program`). The tuner already joins at the program after a break (`timeline::program_from`), so it plays no break clips yet.

### Determinism

Cuts, fill and padding use no randomness. They depend only on the item, its chapters, the two pools and the last stored break clip and its start time, and the shuffle still seeds each program from the channel and the program's first start time. A run resumed at a program boundary, as `extend` always resumes, gives the same events as an unbroken run. The shuffle cooldown counts a cut program once.

## Regeneration

`schedule::regenerate` takes the same arguments as `extend`. Call it when a channel's config or the runtime of an item in its pools changed. Events that start within the next 30 minutes (`schedule::FREEZE_MS`) never change.

- The boundary is the first place at or after `now_ms` plus 30 minutes where the generator can pick up exactly where the stored schedule is: the start of a program (a `Program` event with `media_offset_ms` 0); a `Filler` event that follows another `Filler` event, which is a blocks channel's uncovered time (a break's padding follows a break clip or a program); or time with no events, at its start or at the freeze point if that falls inside it. Every event before the boundary is kept, so a program and its breaks that run past the freeze point are kept whole and the new schedule never starts inside a break. On a blocks channel a new or moved block therefore reaches the filler or dead air after the freeze point, not only the next program.
- In one transaction `Store::replace_events_from` deletes the events from the boundary on and writes the new ones, generated from the boundary to `now_ms` plus seven days. The generator resumes from the stored history before the boundary, exactly as `extend` does: the last program, the shuffle cooldown, each block's place and the last break clip.
- With unchanged inputs the schedule comes out the same. A changed runtime changes only the events from the item's next slot after the boundary; a kept event of that item keeps the runtime it was generated with.
- When the stored schedule ends before any boundary, the time after its end is filled as `extend` fills it.
- If the new events have no program and the replaced ones do, the stored schedule is kept and nothing is written (`written` is 0, and the notes say why, such as `NoPlayableItems`). An empty pool more often means a library fetch or sync failed than that the channel should go dark, so callers should not need their own guard.
- It returns `Extended`, like `extend`, and does nothing without a trusted clock.

## When a file's length differs from its event

The schedule never slips: at play time the scheduled end wins. A kept event, or one generated before a sync corrected an item's runtime, can disagree with the file. `schedule::fit(&event, real_ms, filler)` takes a program event, the file's real length (the library's runtime after a sync, or the length the player reports) and the channel's filler clips, and returns `Fitted { events, fit }`: what plays in the event's time.

- **Short file** (`Fit::Short`). The event plays the media from `media_offset_ms` for its length. If the file ends before that, the program part ends where the file does and the rest, up to the event's end, is filled from the filler pool: its clips from the first, in pool order, whole, over and over, the last one cut at the event's end. A segment the file does not reach at all is all filler. Without a filler clip the rest is empty: the events end where the file ends, and for a segment the file does not reach the list is empty and nothing plays until the segment's end.
- **Long file** (`Fit::Long`). If the event reaches the item's scheduled end (`media_duration_ms`) and the file is longer, the event is unchanged and the file is cut at its end. A segment that ends at a break or at a block's end is cut there anyway, so it is not a mismatch.
- A difference of up to 500 ms either way (`schedule::FIT_TOLERANCE_MS`, the tuner's late tolerance) is `Fit::Exact`: a player or probe often reports a length a few milliseconds off the library's runtime.
- Break, filler and ident events are never fitted.
- `schedule::record_fit(&mut store, channel, &event, fit, at_ms)` writes one playback-history row under the program's media id with the outcome `padded`, `short` (no filler clip) or `cut`, and nothing for `Fit::Exact`. It writes one row per call. The caller (#23) fits and records each program event, so a short file in a program cut by breaks writes one row for each segment it falls short in: the segment where the file ends and every later segment of that program.

`fit` does not change the stored schedule, and the events after it start on time. The tuner does not follow event boundaries yet, so nothing calls `fit` during playback; the playback loop on the Pi (#23) is where it belongs.

## Notes for later work

- A cut program has several events with the same `media_id`. The cooldown walk counts them as one item, but each event uses a row of the history read, so the read needs more pages.
- A run stops with `ScheduleError::TooManyEvents` after 200,000 events, which only happens when runtimes are seconds long.
