# Program guide data

The code is in `src/guide/`. It turns each channel's schedule events into guide cells: one cell per program, for a time window. Drawing the guide is a separate task; this module only supplies the data.

## Calls

| Call | What it does |
| --- | --- |
| `Window::starting_at(now_ms)` | The default window, `now_ms` through three hours later (`DEFAULT_SPAN_MS`). |
| `cells(events, media, window)` | The cells for one channel's events that overlap the window. Pure apart from title lookups through the `MediaSource`. |
| `build(store, media, channels, window)` | One `Row` per channel number, in the order given. Reads the events from the store and calls `cells`. |
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
