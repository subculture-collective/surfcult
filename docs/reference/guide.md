# Program guide data

The code is in `src/guide/`. It turns each channel's schedule events into guide cells: one cell per program, for a time window. On a music channel the guide screens show hour blocks instead of songs ([Music channels](#music-channels)). Drawing the guide is a separate task; this module only supplies the data.

## Calls

| Call | What it does |
| --- | --- |
| `Window::starting_at(now_ms)` | The default window, `now_ms` through three hours later (`DEFAULT_SPAN_MS`). |
| `cells(events, media, window)` | The cells for one channel's events that overlap the window. Pure apart from title lookups through the `MediaSource`. |
| `build(store, media, channels, window)` | One `Row` per channel number, in the order given. Reads the events from the store and calls `cells`. |
| `listings(store, media, channels, timezone, window)` | What the TV guide, `GET /guide` and `GET /xmltv.xml` show: `build`'s cells for a video channel, and hour blocks for a music channel. |
| `blocks(events, media, channel_name, timezone, window)` | A music channel's blocks that overlap the window. |
| `block_start(at_ms, timezone)` | The start of the local hour `at_ms` is in. |
| `Cell::progress(at_ms)` | How far through the cell `at_ms` is, from 0.0 towards 1.0, or `None` when the cell is not on. |
| `Row::current(at_ms)` | The cell on at `at_ms`, if any. |

Nothing reads the clock. Times are Unix epoch milliseconds, and cells are half-open `[start_ms, end_ms)` like events.

## Cells

- A cell has the media id, start, end, title and description. The title comes from `MediaSource::resolve`; it is `None` when the item cannot be resolved, and the cell is still shown. The media model has no description yet, so `description` is always `None`.
- Program events join into one cell when they have the same media id, each starts at the media offset where the one before ended, and only break, filler or ident events fill the time between them, with no hole. The cell runs from the first segment's start to the last segment's end.
- Break, filler and ident events are never cells. A break between two different programs belongs to no cell, so `Row::current` gives `None` there.
- The same item played again from offset 0 is a new cell. So is a segment after a hole or after another program.
- A cell is kept when it overlaps the window. Its start and end are the program's own, not clipped to the window.
- Progress is wall-clock time through the cell, so a break inside the program counts as program time.

## Window edges

`Store::events_between` returns only the events that overlap the window. A program cut by breaks can start before the window or end after it, so `build` reads further one event at a time:

- Backwards while the first event is a break, filler or ident, or a program segment with a media offset above 0.
- Forwards while the last event is a break, filler or ident, or a program segment that stops before the end of its media.
- Either way it stops at a hole in the schedule or at the end of the stored events.

A caller of `cells` that passes its own events must include every segment of a program that crosses an edge, or the cell starts or ends at the first or last segment it was given.

## Music channels

A music channel (one with a `picture`) has one program event per song, two to five minutes long, with a bump between songs. One cell per song would be too narrow for any title, so `listings` groups a music channel's songs into blocks of `MUSIC_BLOCK_MS` (an hour), aligned to the local clock in the config time zone. The schedule is unchanged: the tuner, the banner, `surfcult now`, `schedule --day` and the history still see one event per song, and `build` still gives one cell per song.

- A song belongs to the block of the local hour it starts in, so a song that starts at 13:59 is in the 13:00 block. Each local hour is its own block: the hour a daylight-saving change repeats is two blocks, and the hour it skips is none.
- A block runs from its first song's start to its last song's end. Breaks, filler and idents between its songs are part of it. The clips between two blocks belong to the earlier one, so blocks that follow each other abut and `Row::current` finds a block during a bump.
- A hole in the events (dead air, or the time off the air after a sign-off) ends the block at the last song before it. Songs after the hole in the same hour start a new block.
- The title names the `BLOCK_ARTISTS` (three) artists with the most airtime in the block, as `Santana · The Who · Cream`; ties go to the artist heard first, and a song with two artists counts for both. The row already names the channel, so the title leaves it out, unless no song's artists are known: then it is the channel's name.
- `songs` holds the block's songs as cells, in time order; it is empty for a program. `description` lists the songs' display titles, one a line, and `genres` holds their genres, each once. `media_id` is the first song's.
- To read whole blocks at the window edges, `listings` reads from the start of the block the window starts in (or earlier, when a song crosses into it) to the end of the block it ends in, then on to the next block's first song, so a block's end does not depend on the window.
