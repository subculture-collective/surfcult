# Tuner

The tuner decides what is on screen when the viewer changes channel. The code is in `src/tuner/`. The screen it controls is the `Player` trait in `src/player/`. There is no real MPV control yet (issue #22); everything here runs against `FakePlayer`.

## The sequence

Times are milliseconds after the key press. `L` is the length of the tune-in clip.

```
key press (t=0)   show static (about 0.5 s)
t=500             play a tune-in clip of length L
                  queue the program for time 500+L behind the clip, at its offset;
                  the player opens it early while the clip plays
t=500+L           the clip ends and the queued program starts at the live position
t=500+L+d         the player reports the start; if it was late, one seek to the live position
```

The program is queued behind the clip, not loaded paused, because one MPV process cannot hold a second file paused ([ADR 0001](../adr/0001-pi-and-library-findings.md), decision 5). The offset comes from `timeline::look_ahead`, so it is right for the moment the clip ends.

## How it is driven

The tuner has no clock and never sleeps. Each method takes the time in epoch milliseconds, so tests move a virtual clock by hand. A caller does four things:

| Call | When |
| --- | --- |
| `tune(channel, now)` | the viewer picks a channel |
| `tick(now, store, media)` | the time from `next_wakeup()` arrives |
| `clip_ended()` | the player says the clip reached its end |
| `program_started(now, position_ms)` | the player says the program is on screen |

`pump(now, store, media)` reads the player's pending events with `Player::poll_event`, passes them on, and ticks. `state()` tells where the sequence is: `Static`, `ClipPlaying`, `AwaitingProgram`, `Live`, `Failed` or `Idle`.

The tuner owns a `Player` (the screen). `tick` and `pump` also take a `&Store` (the channel's events) and a `&impl MediaSource` (each program's codecs). In production the caller passes the same `&store` for both and keeps ownership of it, so it can extend schedules and sync the library between calls.

## Choosing the clip

The clip comes from the tune-in pool. It is the shortest clip that is at least as long as the expected start-up cost, or the longest clip if none is. The cost is about 1 s when the program direct-plays and about 7 s when Jellyfin must transcode it (`Timing::direct_start_ms` and `Timing::transcode_start_ms`).

Whether an item needs a transcode is `library::profile::needs_transcode`, which follows ADR 0001, decision 3: direct play for H.265 at 8 or 10 bit and H.264 at 8 bit, up to 1080 pixels tall, with AAC, MP3, FLAC, Opus, AC3, E-AC3, DTS or TrueHD audio. Everything else needs a transcode. `MediaItem` has no HDR flag yet, so HDR cannot be detected.

The program depends on the clip length and the clip depends on the program, so the tuner plans twice at most. It plans with the direct-play cost first. If that program needs a transcode, it plans again with the transcode cost and keeps the second plan.

## A break when the clip ends

If a break, filler or ident event is on when the clip ends, the clip stands in for the break and no second clip plays. The tuner queues the first program event after the break (a break can be several events in a row) at the offset where that event starts. `timeline::program_from` holds this rule. That program starts when the clip ends, so it is early by at most the time left in the break. The tuner does not correct that. The next event boundary puts the screen back on the schedule.

## A late start

When the player reports the program started, the tuner looks up the live position for that moment. If the program is behind it by more than 500 ms (`Timing::late_tolerance_ms`), it seeks to the live position once. It does not seek twice, does not seek backwards, and does not seek when the live event is no longer that program.

## Cancelling

A second `tune` replaces the running sequence. It shows static, which also clears the player's queue, and forgets the plan. The player must not report the end of a clip that a command replaced. For MPV, forward `end-file` with reason `eof` only.

## When there is nothing to show

The tuner leaves static on screen and sets `State::Failed` with a `Reason` when:

- the tune-in pool has no clip (`NoTuneInClip`);
- the channel has no events from now to an hour after the clip ends (`NoEvents`);
- nothing is scheduled when the clip ends, or nothing follows the break (`Gap`);
- the schedule or the media source fails (`Schedule`, `Media`);
- the player rejects a command (`Player`).

It does not retry. Static stays up until the next `tune`.

## What it does not do

- It does not wait for a slow program. A slow transcode keeps the state at `AwaitingProgram`; there is no timeout. Giving up and showing a slate is failure handling (issue #34).
- It does not build playback URLs or ask Jellyfin to transcode (issue #21).
- It does not follow the schedule after the program has started. Moving to the next event is the player's loop (issue #23).
