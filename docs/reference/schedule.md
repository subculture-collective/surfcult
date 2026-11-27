# Schedule generator

The code is in `src/schedule/`. It turns pools of media items into `timeline::Event`s for one channel: `sequential`, `shuffle` and `weighted` channels and `blocks` channels, programs back to back with commercial breaks, seven days ahead, with an optional nightly sign-off, and regenerates the future after a change. Grids are not done yet.

## Two layers

`generate(&Params, sources, &Resume, start_ms, end_ms)` is a pure function inside the crate. It reads no clock and does no I/O. `sources` is a list of `SourcePool`s: each source with its weight (used by `weighted`) and its pool of items; every other mode plays the first. `Params` holds the channel number, the `Mode` and an optional `BreakPlan` (the break interval and length, the break pool's clips, rotation and repeat limit, the pad pool's clips and the filler pool's clips). `Resume` holds `recent`, the media ids of earlier programs, newest first, of which the first is the cursor, the last item scheduled; `last_break`, the start time and media id of the last break clip; `aired`, the start times and media ids of the recent break clips, newest first; and `last_event`, the end time and kind of the channel's last stored event, which only a channel with a sign-off reads. It returns `Generated { events, cursor, notes }`. Times are Unix epoch milliseconds.

`generate_blocks(&Layout, &Resume, start_ms, end_ms)` is the same for a blocks channel. `Layout` holds the channel number, the `BreakPlan`, one `Lane` (a mode and its `SourcePool`s) per block in config order and then one for the default if there is one, the default's index, the spans from `blocks::spans(blocks, timezone, start_ms, end_ms)`, the channel's filler clips, its nights off the air and its sign-off clip. `spans` is pure too: it turns the blocks' local times into epoch instants. A channel with one source and a sign-off goes through `generate_fixed(&Layout, ...)`: one lane, one span over all time, and the same filler, nights and clip. The nights come from `sign_off::nights(sign_off, timezone, start_ms, end_ms)` and the clip from `sign_off::clip`; see Sign-off.

`schedule::extend(&mut Store, &Channel, pools, clips, timezone, now_ms, clock_trusted)` keeps a channel's week filled. `clips` is the scanned clip pools, `ClipLibrary::pools()`; a channel finds its break pool, its `breaks.pad` pool, its `filler` pool and its `sign_off` pool there by name, and a missing pool counts as empty. The caller supplies the program pools as `(Source, Vec<MediaItem>)` pairs: it resolves each source of `channel.plan.sources()` with `library::resolve_pool(channel.number, source, &store)` and passes the pairs to `extend`. A source with no pair counts as an empty pool. `timezone` is the config's `timezone`; blocks channels and channels with a sign-off read it. Every source kind resolves; a source that matches nothing is an error naming the channel and the source, so a channel never falls back to the whole library. The order each source gives is in [Jellyfin source](jellyfin.md#channel-sources). `extend` returns `Extended::ClockNotTrusted` and touches nothing when `clock_trusted` is false, because a Pi with an unsynchronized clock must not store a wrong week. Otherwise it starts at the later of `now_ms` and the end of the last stored event and generates up to `now_ms` plus seven days (`HORIZON_MS`). If the schedule already reaches that far it returns `Extended::UpToDate`. It never reads the system clock.

The resume point comes from the stored events, not from a separate cursor. The cursor is the media id of the channel's last program event that starts before the new window, at any time, past or future. A sequential channel reads only that one event (`Store::last_program_before`). A shuffle or weighted channel reads its most recent program events, newest first, in pages of twice the pools' total length (`Store::recent_programs_before`, which walks back on the primary key). It reads the next page, bounded by the oldest start time so far, until the rows hold the N - k distinct items cooling down in every pool (one for a shuffle, one per source for a weighted channel; see Shuffle and Weighted) or the history ends, and passes the rows as `Resume::recent`. One page is usually enough; the read reaches further back only when the recent programs are mostly ids no longer in the pool, or when the history is shorter than N - k distinct pool items, in which case it reads the whole history. Writing the new events is the only write and is one transaction, so a crash cannot leave the schedule and the resume point disagreeing. Because the resume point is read from the events, `Store::replace_events_from` also moves it back to the last program it keeps. A channel with breaks also reads its stored break events that start in the window before the new one, newest first (`Store::breaks_between`, which walks back on the primary key). The window is 24 hours (`ROTATION_MEMORY_MS`) or the break pool's `repeat_after`, whichever is longer. The rows are `Resume::aired`, which the repeat limit counts from, and the newest is `Resume::last_break`, where the rotation continues. The read stops at that bound, so a channel that never stores a break does not walk its whole history on every extend.

## Rules for every run

- Without breaks, each program is one `Program` event with `media_offset_ms` 0 and `media_duration_ms` set to the item's runtime. It starts where the last one ended.
- The events cover the whole window with no gap and no overlap. The last program may end after the window. A program cut by breaks is written whole, so a run always ends on a program's last segment.
- An item with a runtime of 0 is skipped. An id that appears twice in the pool is used once.
- A pool with nothing playable gives no events and `Note::NoPlayableItems`. Nothing is written, so the resume point stays as it was.
- A `grid` setting on the channel is ignored for now.

A blocks channel differs in two ways: a program still on at the end of its block is cut there, and time with nothing to play has no events. See Blocks. A channel with a sign-off has no events while it is off the air, and none in the gap before the sign-off when it has no filler. See Sign-off.

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

A weighted lineup has several sources, each with a weight. Each program slot first picks a source with probability proportional to its weight, then picks an item from that source's pool with the shuffle's cooldown.

```toml
mode = "weighted"
sources = [
  { jellyfin_series = "The Simpsons", weight = 3 },
  { jellyfin_series = "Futurama", weight = 1 },
  { jellyfin_collection = "Cartoon Shorts", weight = 1 },
]
```

- `sources` is how a weighted fixed channel, block or `default` names what it plays; it takes the place of `source`, which a weighted lineup does not accept. It needs at least one entry. Each entry is one source key (`jellyfin_library`, `jellyfin_collection`, `jellyfin_tag`, `jellyfin_genre` or `jellyfin_series`) and an optional `weight`, a whole number from 1 to 4294967295 that is 1 when left out. The same source twice in one list is a config error, as is any other key. Every other mode takes one `source` and rejects `sources`. Config errors name the field, for example `channel[0].block[1].sources[2].weight`.
- A source's share of the programs approaches its weight divided by the total weight, however many items its pool holds. A source with 40 items and a source with 1 item of equal weight play equally often.
- The pick uses one SplitMix64 per slot, seeded from the channel number and the slot's `start_ms`, as the shuffle does. Its first draw is a number below the total weight; the first source, in config order, whose running total of weights is above it is picked. Its next draw picks an index among the source's eligible items, in pool order. The same inputs give the same picks on every machine.
- Each source's pool has its own cooldown, by the shuffle's rule: with N distinct playable items in the pool and k = max(1, ceil(N / 4)), the N - k most recently played distinct items of that pool are cooling down and are not picked from it. The history is the earlier programs, from any source, that are in the pool, so no item repeats within N - k + 1 consecutive programs of its pool, and a source with one item plays it whenever it is picked. A program from another source does not use up the cooldown of a pool that does not hold it, so a rarely picked source keeps its place. An item that two sources share cools down in both.
- A source with no playable item (an empty pool, or items with no runtime) is left out of the draw, and `Note::SourceNotPlayable { source }` names it. The other sources share the picks by their weights. When none is playable the lane has nothing to play: `Note::NoPlayableItems`, and no events on a fixed channel.
- `extend` reads back until every source's pool has its N - k cooling items or the history ends, with the same paging as a shuffle, so a run that stops at an event boundary and resumes gives what an unbroken run would. A caller of `generate` that passes a shorter history loses that guarantee.

Limits:

- There are no passes: a source with a high weight can play several times before a source with a low one plays at all, and an item can play again before another item of its source has played once.
- If the whole stored history holds fewer than N - k distinct items of a pool, for example on a new channel or after a source changed, fewer items of it are cooling down.
- Weights count programs, not minutes, so a source of long items takes more of the air than its weight suggests.

## Blocks

A `blocks` channel lists `[[channel.block]]` entries, each with `days`, `start`, `end`, a `source` and a `mode`, or, for `weighted`, `sources` in place of `source` (see Weighted). It may also have a `default = { source, mode }` or `default = { sources, mode }`, which plays the time no block covers.

### Where a block falls

- Block times are local wall-clock times in the config's `timezone`. Each block covers `[start, end)` on each of its days; `days` are the days it starts on, every day when left out.
- The spans hold real instants, so a block from 00:00 to 24:00 lasts 23 hours on the day the clocks go forward and 25 on the day they go back. A block from 06:00 to 12:00 in Chicago starts at 11:00 UTC in summer and 12:00 UTC in winter, including on both change days.
- A local time that happens twice, when the clocks go back, means its first occurrence. A local time that does not happen, when the clocks go forward, means the moment they jump: in Chicago on the spring change, 02:30 means 03:00 CDT. A block wholly inside the skipped hour does not play that day.

### What plays

- A block plays its own pool in its own mode. Its first program starts at the block's start.
- A program still on at the block's end is cut there: the event ends at the boundary and keeps the item's runtime as `media_duration_ms`. A break or clip on at that moment is cut the same way, and break clips after it are dropped. Breaks that would start at or after the block's end are not filled and do not count toward `Note::ShortBreaks`. The break rotation continues after the last break clip that was kept, and dropped clips do not count toward `repeat_after`.
- Time no block covers plays the `default` when there is one. Otherwise it plays the channel's `filler` pool: its clips in pool order, over and over, counted from the start of the uncovered stretch, with the last clip cut at the stretch's end. Because the cycle is counted from the stretch's start, a run that resumes in the middle of it plays the same clips.
- Such a filler stretch can last hours, unlike a break. The tuner does not play filler clips, and it does not queue the next block's first program early: when filler is on as the tune-in clip ends and the next program starts more than 10 minutes later, it shows the stand-by card with `Reason::NoProgramSoon` ([tuner.md](tuner.md), "A break when the clip ends"). The run loop's playback driver then plays the filler from its next clip ([run.md](run.md#the-playback-driver)).
- With no default and no filler clip, uncovered time has no events. The tuner treats that time as a gap ([tuner.md](tuner.md), "When there is nothing to show"). `Note::DeadAir { ms }` says how much there was.
- A block whose pool has nothing playable falls back the same way, and `Note::NoPlayableItems` is reported.

### Keeping each block's place

- Each block, and the default, keeps its own place. A sequential block continues after the newest program, anywhere in the channel's history, that is in its pool, so Saturday's cartoons continue from last Saturday's. A shuffle block's cooldown counts only programs from its pool, and a weighted block's counts only programs from each source's pool.
- A program counts for every block whose pool holds it, so two blocks that share items share their place.
- `extend` reads the channel's programs, newest first, in pages of twice the pools' total length, until each sequential block has one program from its pool and each shuffle block, and each source of a weighted block, the N - k distinct items it keeps cooling down, or the history ends. A block that has never played, such as a new one, makes it read the whole history.
- A run that resumes at an event boundary gives what an unbroken run would, across days and clock changes.

## Sign-off

A channel with `sign_off = { at, pool, clip, until }` ends each night with one clip, such as the national anthem, and is then off the air until `until`. It works on every kind of channel; on a blocks channel the time off the air overrides its blocks and default.

### When

- `at` is the local time the sign-off clip starts and `until` the local time the channel comes back on the air, both `HH:MM` from 00:00 to 23:59 in the config's `timezone`. `until` is the next morning when it is not after `at` (23:30 to 06:00) and the same day otherwise (01:00 to 05:30). The two must differ.
- Times follow the block rules: a time that occurs twice is its first occurrence, and a time that does not occur is the moment the clocks jump. In Chicago, 23:00 to 06:00 lasts six hours on the night the clocks go forward and eight on the night they go back.

### Which clip

- `pool` must name an `[[interstitial_pool]]`. `clip`, if given, is a plain file name in it, with no path; the scan resolves links, so it is the name of the file a link points to.
- The clip is the named file when the scanned pool has it, and otherwise the pool's first clip in pool order, so the sign-off is the same every night.
- If the pool has no usable clip, the channel still goes off the air at `at`, with no clip, and `Note::NoSignOffClip` says so.

### What plays

- No program, and so none of its breaks, starts if it would still be on at `at`. The generator writes each program before the sign-off whole and takes it back if it ends after `at`. On a sequential channel, or a blocks channel whose `until` falls in the same sequential lane, the program it took back is the next one after the night, so nothing is skipped. Shuffle and weighted picks are seeded by time, so the pick at `until` is a new draw.
- The gap from the last program's end to `at` is filled from the channel's `filler` pool: its clips in pool order, whole, over and over, counted back from `at`, so the last clip ends as the sign-off starts and the first one may start part-way in. Uncovered time on a blocks channel counts its cycle forward from the start of the time instead; counting back from `at` makes what plays in the gap independent of where the gap began. Without filler the gap is dead air, counted in `Note::DeadAir`.
- The sign-off clip is one `Ident` event that starts at `at` and runs its full length, or until `until` if it is longer than the time off the air.
- From the end of the clip until `until` the channel has no events. The tuner shows static, with `Reason::NoEvents` or `Reason::Gap` ([tuner.md](tuner.md), "When there is nothing to show").
- At `until` the channel's plan resumes: a fixed channel continues its order, a blocks channel plays whatever block or default covers `until`. A sequential channel continues after its last program, the shuffle cooldown counts the programs before the night, and the break rotation follows its usual rules.

### Resuming

A run that starts inside the stretch before a sign-off reads the channel's last stored event (`Store::last_event_before`). If that event is a program that ends where the run starts, the run places programs as usual. If it is filler, the run is in the gap and plays filler or dead air until `at`. If it is a program that ended earlier in the stretch, the run first tries the next program from that program's end: if that program would still be on at `at`, the gap began there and the run is in it; otherwise the run places programs from where it starts. If no stored event ends inside the stretch, because the last one is the sign-off clip of the night before or a program cut at the end of the block before, the run tries the first program from the start of the stretch in the same way. Either probe covers two cases: a regeneration from the freeze point inside dead air, and an `extend` after the schedule ran out while the Pi was off, which does not fill the rest of the day with filler. A run that starts at the sign-off, in its clip, off the air or at `until` needs nothing more than the time. So `extend` in two parts gives what one run would, and a regeneration with unchanged inputs changes nothing, wherever the boundary falls.

This holds while the sign-off settings stay the same. After `sign_off` is added or moved, the events kept before a regeneration's boundary can include a program that runs past the new `at`, and a stored schedule can end inside a new night. A run that starts inside a night plays only the part of the clip still to come, or nothing once the clip would have ended, so that night has a late, partial sign-off clip or none. Later nights are correct.

## Breaks

A channel with `breaks = { every, length, pool }` (and optionally `pad`) has its programs cut, and each cut filled with a break. Breaks add to the slot: a 22-minute program with two 2-minute breaks occupies 26 minutes, as three program events and the break events between them.

### Where a program is cut

- The nominal cuts are the multiples of `every` in the program's own time, strictly inside its runtime. There is no cut at the end; the break between programs is grid work.
- Each cut moves to the nearest Jellyfin chapter mark within 60 seconds (`schedule::SNAP_MS`), the earlier mark on a tie, as long as the mark is after the previous cut and before the end. With no such mark the cut stays at the interval.
- Every program event of a cut item has the item's `media_id` and runtime as `media_duration_ms`. The first starts at offset 0 and each later one at the previous cut, so a tune-in seek is always "this media at this offset".

### What fills a break

- The break pool's clips are walked once, in the order its `rotation` gives (see below). Each clip that fits in the time left and is not held back by the pool's `repeat_after` is placed as one `Break` event as long as the clip, rounded up to a whole millisecond. A clip is never cut and never plays twice in one break. A clip longer than the time left is skipped and waits for a later break.
- The time left over is one pad event exactly as long as the shortfall. It comes from the `breaks.pad` pool as an `Ident` event when that pool has a clip, and otherwise from the channel's `filler` pool as a `Filler` event. Either way it plays the pool's shortest clip at least that long, or its longest clip if none is long enough. The pad is the only piece that may stop before its clip ends, or run past it, so both pools should hold idents and slates. The pad ignores the pad pool's own `rotation` and `repeat_after`, and config validation refuses those keys on a pool that no channel uses as `breaks.pool`.
- Without a usable pad or filler clip a break comes out shorter than `length`. A break with nothing to show at all is left out and the program runs on uncut. Both count toward `Note::ShortBreaks { count }`.

### Rotation and frequency

Each `[[interstitial_pool]]` takes two optional keys that apply when it is a channel's break pool. A clip plays at most once per break in every case.

- `rotation = "order"` (the default): clips rotate in pool order from break to break. The rotation continues after the channel's last stored break clip; if that clip is no longer in the pool it starts again at the first clip. The rotation remembers its place for 24 hours (`ROTATION_MEMORY_MS`): a break that starts more than 24 hours after the last break clip started begins at the first clip.
- `rotation = "shuffle"`: each break walks the pool in a random order drawn with SplitMix64 seeded from the channel number and the break's start time. Nothing else goes into the seed, so the order does not depend on where a run started.
- `repeat_after = "<duration>"` (for example `"2h"`; no default): on one channel, a clip does not play again until that long after its last airing started. A clip still cooling down is skipped. When no clip fits, the break is padded, so the limit always holds. It must be longer than zero. Without it, once per break is the only limit.

`extend` reads the channel's break events back to the longer of 24 hours and `repeat_after`, and an unbroken run applies the same rules, so a resumed run matches an unbroken one. A break clip cut off by the end of a block never airs and does not count toward the limit. Clips that share a media id, such as one file reached through two symlinks, count once, at the first one's place in the pool.

### Clip ids

A clip's `media_id` is its canonical path as text (`schedule::clip_media_id`). Clips whose path is not valid UTF-8 are left out. The event kind tells a clip (`Break`, `Filler`, `Ident`) from a Jellyfin item (`Program`). The tuner joins at the program after a break (`timeline::program_from`); the run loop's playback driver plays break, filler and ident clips by this id ([run.md](run.md#the-playback-driver)).

### Determinism

Cuts, an `order` rotation and padding use no randomness. They depend only on the item, its chapters, the pools and the stored break clips and their start times. A `shuffle` rotation seeds each break from the channel and the break's start time, and the program shuffle still seeds each program from the channel and the program's first start time. A run resumed at a program boundary, as `extend` always resumes, gives the same events as an unbroken run. The shuffle cooldown counts a cut program once.

## Regeneration

`schedule::regenerate` takes the same arguments as `extend`. Call it when a channel's config changed; after a library sync, `reschedule` calls it when needed. Events that start within the next 30 minutes (`schedule::FREEZE_MS`) never change.

- The boundary is the first place at or after `now_ms` plus 30 minutes where the generator can pick up exactly where the stored schedule is: the start of a program (a `Program` event with `media_offset_ms` 0); a `Filler` event that follows another `Filler` event, which is a blocks channel's uncovered time or the gap before a sign-off (a break's padding follows a break clip or a program); an `Ident` event that starts when a night off the air starts, which is the sign-off clip; or time with no events, such as dead air or the time off the air, at its start or at the freeze point if that falls inside it. Every event before the boundary is kept, so a program and its breaks that run past the freeze point are kept whole and the new schedule never starts inside a break. On a blocks channel a new or moved block therefore reaches the filler or dead air after the freeze point, not only the next program.
- In one transaction `Store::replace_events_from` deletes the events from the boundary on and writes the new ones, generated from the boundary to `now_ms` plus seven days. The generator resumes from the stored history before the boundary, exactly as `extend` does: the last program, the shuffle cooldown, each block's place and the recent break clips.
- With unchanged inputs the schedule comes out the same. A changed runtime changes only the events from the item's next slot after the boundary; a kept event of that item keeps the runtime it was generated with.
- When the stored schedule ends before any boundary, the time after its end is filled as `extend` fills it.
- A pool with nothing playable replaces the future as well. From the boundary on the channel gets filler, or no events (dead air), and the notes say why, such as `NoPlayableItems`. The tuner shows the stand-by card for that time. This is never the result of a failed library fetch: a failed sync leaves the library cache as it was, and a source that matches nothing is an error before `regenerate` runs. So an empty pool comes from a deliberate config change, and `regenerate` follows it.
- It returns `Extended`, like `extend`, and does nothing without a trusted clock.

### Following library changes

`schedule::reschedule(&mut Store, &Channel, pools, clips, timezone, now_ms, &changes)` keeps a channel in step with a library sync. `changes` is the `LibraryChanges` that `Store::replace_library` returned (see [Jellyfin source](jellyfin.md#keeping-the-schedule-in-step)), and `pools` are resolved from the cache after the sync, so they have the new runtimes and not the items that left. The caller checks that the clock is trusted.

- If an event from `now_ms` on plays an item whose runtime changed (`resized`) or that left the library (`vanished`), the channel is regenerated as above. Events within the freeze point stay, including a program of an item that left; that program fails when it is due and the tuner shows the stand-by card.
- Each program of a vanished item that the regeneration replaced is recorded with `Store::record_substitution`: the channel, the item, the time it was to start and the media of the event that the new schedule has at that time (`None` for dead air). `Store::substitutions(channel)` reads them back.
- Otherwise the schedule is only extended.
- A renamed item (a new id for the same file path) needs nothing here: `replace_library` already gave its events the new id.

## When a file's length differs from its event

The schedule never slips: at play time the scheduled end wins. A kept event, or one generated before a sync corrected an item's runtime, can disagree with the file. `schedule::fit(&event, real_ms, filler)` takes a program event, the file's real length (the library's runtime after a sync, or the length the player reports) and the channel's filler clips, and returns `Fitted { events, fit }`: what plays in the event's time.

- **Short file** (`Fit::Short`). The event plays the media from `media_offset_ms` for its length. If the file ends before that, the program part ends where the file does and the rest, up to the event's end, is filled from the filler pool: its clips from the first, in pool order, whole, over and over, the last one cut at the event's end. A segment the file does not reach at all is all filler. Without a filler clip the rest is empty: the events end where the file ends, and for a segment the file does not reach the list is empty and nothing plays until the segment's end.
- **Long file** (`Fit::Long`). If the event reaches the item's scheduled end (`media_duration_ms`) and the file is longer, the event is unchanged and the file is cut at its end. A segment that ends at a break or at a block's end is cut there anyway, so it is not a mismatch.
- A difference of up to 500 ms either way (`schedule::FIT_TOLERANCE_MS`, the tuner's late tolerance) is `Fit::Exact`: a player or probe often reports a length a few milliseconds off the library's runtime.
- Break, filler and ident events are never fitted.
- `schedule::record_fit(&mut store, channel, &event, fit, at_ms)` writes one playback-history row under the program's media id with the outcome `padded`, `short` (no filler clip) or `cut`, and nothing for `Fit::Exact`. It writes one row per call. A caller fits and records each program event, so a short file in a program cut by breaks writes one row for each segment it falls short in: the segment where the file ends and every later segment of that program.

`fit` does not change the stored schedule, and the events after it start on time. Nothing calls `fit` during playback yet. The playback driver (`src/run/driver.rs`) is where it belongs, but it needs the file's real length, which only the player knows once it has opened the file; until then a short file leaves the screen black until its event ends and a long one is cut there ([run.md](run.md#the-playback-driver)).

## Notes for later work

- A cut program has several events with the same `media_id`. The cooldown walk counts them as one item, but each event uses a row of the history read, so the read needs more pages.
- A run stops with `ScheduleError::TooManyEvents` after 200,000 events, which only happens when runtimes are seconds long.
