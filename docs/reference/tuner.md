# Tuner

The tuner decides what is on screen when the viewer changes channel. The code is in `src/tuner/`. The screen it controls is the `Player` trait in `src/player/` ([player reference](player.md)). The tests here run against `FakePlayer`; `MpvPlayer` is the real one.

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

The program is queued behind the clip, not loaded paused, because one MPV process cannot hold a second file paused ([ADR 0001](../adr/0001-pi-and-library-findings.md), decision 5). The program and its offset come from `timeline::program_from` at the moment the clip ends, so they are right for that moment. If a break is on then, the program after the break is queued at its event's `media_offset_ms`.

## How it is driven

The tuner has no clock and never sleeps. Each method takes the time in epoch milliseconds, so tests move a virtual clock by hand. A caller does four things:

| Call | When |
| --- | --- |
| `tune(channel, now)` | the viewer picks a channel |
| `tick(now, store, media)` | the time from `next_wakeup()` arrives |
| `clip_ended()` | the player says the clip reached its end |
| `program_started(now, position_ms)` | the player says the program is on screen |

`restarted(now)` is a fifth call, for when the player says it restarted MPV.

`pump(now, store, media)` reads the player's pending events with `Player::poll_event`, passes them on, and ticks. `state()` tells where the sequence is: `Static`, `ClipPlaying`, `AwaitingProgram`, `Live`, `Failed` or `Idle`.

The tuner owns a `Player` (the screen). `tick` and `pump` also take a `&Store` (the channel's events) and a `&impl MediaSource` (each program's codecs). In production the caller passes the same `&store` for both and keeps ownership of it, so it can extend schedules and sync the library between calls.

## Choosing the clip

The clip comes from the tune-in pool. It is the shortest clip that is at least as long as the expected start-up cost, or the longest clip if none is. The cost is about 1 s when the program direct-plays and about 7 s when Jellyfin must transcode it (`Timing::direct_start_ms` and `Timing::transcode_start_ms`).

Whether an item needs a transcode is `library::profile::needs_transcode`, which follows ADR 0001, decision 3: direct play for H.265 at 8 or 10 bit and H.264 at 8 bit, up to 1080 pixels tall, with AAC, MP3, FLAC, Opus, AC3, E-AC3, DTS or TrueHD audio. Everything else needs a transcode. The rules are constants in `src/library/profile.rs`, and the device profile sent to Jellyfin is built from the same constants (see `jellyfin.md`). `MediaItem` has no HDR flag yet, so HDR cannot be detected here, although Jellyfin transcodes it.

The program depends on the clip length and the clip depends on the program, so the tuner plans twice at most. It plans with the direct-play cost first. If that program needs a transcode, it plans again with the transcode cost and keeps the second plan.

## A break when the clip ends

If a break, filler or ident event is on when the clip ends, the clip stands in for the break and no second clip plays. The tuner queues the first program event after the break (a break can be several events in a row) at the offset where that event starts. `timeline::program_from` holds this rule. That program starts when the clip ends, so it is early by at most the time left in the break. The tuner does not correct that. The next event boundary puts the screen back on the schedule.

A filler stretch can be much longer than a break: a blocks channel with no default plays its filler pool through the time no block covers ([schedule.md](schedule.md), "What plays"). So the tuner queues the program after the run only if it starts at most 10 minutes after the clip ends (`Timing::max_early_ms`). Otherwise it leaves static up with `NoProgramSoon { next_program_ms }`; it cannot play the filler clips itself yet. A stretch that runs past the hour of schedule the tuner reads has no program in view and ends on `Gap` with no next start. A break longer than 10 minutes is treated the same way.

## A late start

When the player reports the program started, the tuner looks up the live position for that moment. If the program is behind it by more than 500 ms (`Timing::late_tolerance_ms`), it seeks to the live position once. It does not seek twice, does not seek backwards, and does not seek when the live event is no longer that program.

## Cancelling

A second `tune` replaces the running sequence. It shows static, which also clears the player's queue, and forgets the plan. The player must not report the end of a clip that a command replaced. For MPV, forward `end-file` with reason `eof` only.

## When there is nothing to show

The tuner leaves static on screen and sets `State::Failed` with a `Reason` when:

- the tune-in pool has no clip (`NoTuneInClip`);
- the channel has no events from now to an hour after the clip ends (`NoEvents`);
- nothing is scheduled when the clip ends, or nothing follows the break (`Gap`);
- a break, filler or ident run is on when the clip ends and the next program starts more than `Timing::max_early_ms` later (`NoProgramSoon`);
- the schedule or the media source fails (`Schedule`, `Media`);
- the player rejects a command (`Player`).

It does not retry. Static stays up until the next `tune`.

## When the player restarts

When the player reports `Restarted` (MPV died and the watchdog started a new one), the tuner tunes the current channel again, as if the viewer had picked it: static, a tune-in clip, then the program at its live position. This also happens from `Failed`, since a dead MPV is a common cause of a player failure. Nothing happens when the tuner is `Idle`. The schedule is in the store, so nothing about it is lost.

## What it does not do

- It does not wait for a slow program. A slow transcode keeps the state at `AwaitingProgram`; there is no timeout. Giving up and showing a slate is failure handling (issue #34).
- It does not build playback URLs or ask Jellyfin to transcode (issue #21).
- It does not follow the schedule after the program has started. Moving to the next event is the player's loop (issue #23).
