# surfcult design

Status: draft for approval, 2026-11-22. This replaces the technical choices in [`docs/idea.md`](../../idea.md), which stays as the original pitch. Where the two disagree, this document wins.

## Intent

surfcult turns a Raspberry Pi (or any Linux box) into a cable-TV set. Your own media plays as channels that keep running whether or not anyone is watching. Turning the TV on or changing channel drops you into whatever is "on" right now, partway through, with static, commercials and a program guide around it.

The owner already runs Jellyfin for the library and Tdarr to convert it to H.265. surfcult does not replace either. It decides what is on when, and plays it.

## Goals, non-goals, anti-goals

Goals:

- Channel changes join the broadcast in progress. A show never restarts because someone switched away and back.
- Every channel has one deterministic timeline, so the guide, the screen and a restart all agree.
- Runs on a Raspberry Pi 4B (2 GB) with no transcoding on the Pi.
- Works with a half-converted library: H.265 plays directly, anything else still plays.
- Operated with a remote control alone.

Non-goals (v1):

- A media server, metadata scraper or downloader. Jellyfin owns the library.
- Transcoding inside surfcult. Jellyfin transcodes when the Pi cannot decode a file.
- Streaming channels to other devices (IPTV, HLS). Only the watched channel plays.
- Several screens, live IPTV sources, analog output.

Anti-goals (a failure even if it works):

- It feels like a media library with a TV skin: menus, pick-an-episode, play-from-start.
- A channel change that shows a black screen or a spinner for several seconds.
- A schedule that visibly changes under the viewer, such as the guide rewriting the show that is on now.

## Constraints

- Hardware: the target is the owner's Raspberry Pi 4B with 2 GB of RAM. It should decode H.265 and H.264 in hardware; this needs checking on the device (see Risks). A Pi 5 has no H.264 hardware decoder, so it is untested and not a v1 target. With 2 GB, MPV caches stay small and nothing else heavy runs on the box.
- Display: the TV is 4K, but most of the library is 1080p. v1 outputs 1080p and lets the TV upscale, which keeps the overlay bitmaps and compositing cheap on a Pi 4. 4K output is a later option.
- Library: mostly H.265, with Tdarr converting the rest. Tdarr replaces files in place, so a file's duration can change after it was scheduled.
- Network: the Pi reaches Jellyfin over the LAN. "Offline" here means no internet. It does not mean no LAN.
- Secrets: the Jellyfin API key comes from the environment or a secret file. It never goes in the repository, a URL or a log.
- Time: store UTC, define schedules in a named time zone, and handle days with 23 or 25 hours.
- Clock: a Pi has no battery clock. The design assumes the LAN provides NTP (for example from the router or Jellyfin host). Until the system clock reports as synchronized, the tuner shows a slate and the scheduler generates nothing, so a stale clock cannot freeze a wrong week.

## Approaches considered

1. **Shared stream per channel** (the idea.md addendum). FFmpeg produces one MPEG-TS stream per active channel and the screen is a client of it. Rejected: it needs timestamp and keyframe handling the original document itself calls hard, costs CPU on a Pi, and serves IPTV clients nobody has asked for.
2. **Own media index, files from a mounted share.** Rejected for v1: it repeats what Jellyfin already knows (metadata, durations, codec fallback) and leaves transcoding to the Pi. Kept as a later `MediaSource` implementation.
3. **One player, Jellyfin as source, precomputed schedule** (chosen). One MPV process plays only the watched channel. Jellyfin supplies the library and decides between direct play and transcode. The scheduler is independent of the player and could feed other outputs later.

The screen UI is also a choice. A browser kiosk over the video was rejected because layering a browser over MPV on a Pi is fragile and Chromium's hardware H.265 support is uneven. MPV draws the TV screen itself.

These choices are the author's, made on the owner's instruction to design it as seen fit. The owner chose the per-channel clock, one transcode at a time, static between channels and short on-disk clips to cover a tune-in.

## Design

### Shape

One Rust binary with modules, not a workspace of crates. Split a module into a crate only when a real boundary hurts.

```
channels.toml ──> scheduler ──> SQLite (events) ──┐
Jellyfin API  ──> library cache ──────────────────┤
                                                  v
remote/keys ──> tuner ──> player (MPV, JSON IPC) ──> HDMI
                  │             ^
                  └── overlay ──┘   (banner, static, guide)
axum web admin and API (later) reads and writes the same state
```

Modules: `timeline` (pure functions, no I/O), `schedule` (generator), `library` (the `MediaSource` trait plus a Jellyfin implementation and a SQLite cache), `player` (MPV process and IPC), `overlay` (TV screen graphics), `tuner` (channel state machine), `config`, `server`.

### Configuration and storage

`channels.toml` is the source of truth for channels, rules and breaks. It lives in a file you can keep in git. SQLite holds derived and runtime state: the library cache, generated events, playback history and substitutions. The web admin, when it exists, edits the same TOML.

```toml
timezone = "America/Chicago"

[[channel]]
number = 3
name = "SATURDAY CARTOONS"
source = { jellyfin_collection = "Cartoons" }
mode = "shuffle"          # sequential | shuffle | weighted | blocks
grid = "30m"              # programs start on the half hour; gaps are filled
breaks = { every = "11m", length = "2m", pool = "ads-90s" }
filler = "filler"         # pool used to fill grid gaps

[[interstitial_pool]]
name = "tune-in"
path = "/var/lib/surfcult/clips/tune-in"   # short local clips
```

### Schedule

Each channel has a materialized, persisted list of events: start, end, media id, the offset into that media, the media duration at the time of generation, and kind (program, break, filler, ident). A program that is interrupted by breaks becomes several events with increasing offsets, so a tune-in seek is always "this media at this offset". The generator fills a rolling seven days ahead.

Content sources and modes (v1):

- Sources: a Jellyfin library, collection, tag or genre, or a single series. Collections and series expand to their episodes and movies.
- `sequential`: plays items in order and remembers its place. The next item is kept as a cursor per channel in SQLite and advances as events are generated, so regenerating the future continues from the cursor at the frozen boundary.
- `shuffle`: seeded random order without repeats until the pool is used up.
- `weighted`: seeded random pick where each source or item carries a weight.
- `blocks`: a list of time-of-day (and day-of-week) blocks, each with its own source and mode, for things like Saturday-morning cartoons. Later blocks in the file do not override earlier ones; overlapping blocks are a config error.

Programs and breaks:

- Breaks cut into programs, as on television. Cut points fall at `breaks.every` by default and snap to the nearest Jellyfin chapter mark within 60 seconds when one exists, so cuts are less likely to land mid-scene.
- Breaks add to the slot. A 22-minute episode with two 2-minute breaks occupies 26 minutes.
- Intros and credits are not detected in v1. "What is on now" is a lookup of the event containing `now`, so a channel costs nothing while nobody watches it.

- Randomness is seeded from the channel and date, so regenerating a day gives the same order unless its inputs changed.
- With `grid` set, programs start on the grid and any gap is filled from the break pool, then idents, then the channel's `filler` pool. Looping a single ident is a last resort. Channels with long, irregular programs such as movies should leave `grid` off.
- The schedule never slips. At an event boundary the scheduled end wins: if a file ends early, the rest of the event is filled from the filler pool; if it runs long, it is cut at the scheduled end. Every such mismatch is recorded, and the next library sync corrects the stored duration so later events are generated from the right length.
- Events starting within the next 30 minutes are frozen. Anything later may be regenerated, for example when Tdarr changes a file's duration or when the owner edits a channel. The show on screen and the next few guide rows never change under the viewer.
- A file that disappears from Jellyfin is replaced by valid content and the substitution is recorded.
- Items do not repeat within the same day unless the pool is too small to avoid it.

### Tuning

Changing channel runs a fixed sequence. The scheduler looks ahead, so the seek position is correct for when the clip ends, not for when the button was pressed.

```
t=0     key press; show static (~0.5 s)
t=0.5   play a local tune-in clip of length L
        meanwhile the program for time (0.5 + L) is loaded paused at its offset
t=0.5+L program starts at the live position
```

- If the channel is already in a break at `0.5 + L`, the clip stands in for the break and no extra clip is played.
- L is chosen from the expected start-up cost: a short clip for direct play, a longer one when Jellyfin must transcode first.
- If the program starts late, one correction seek puts it back on the live position.
- Switching again during the sequence cancels it and starts the sequence for the new channel.

### Breaks and interstitials

Breaks are filled from pools of local clips (ads, station idents, bumpers, previews, emergency slates). The generator fills a break up to its target length with whole clips, then pads with an ident or slate. It never cuts a clip. Rotation and frequency limits apply per pool. Clips live on the Pi's own storage, so they play instantly and do not depend on the network.

### Media source: Jellyfin

- The library sync reads items with runtime, codecs and streams, and caches them in SQLite.
- Before playing, surfcult asks Jellyfin's playback-info endpoint with a device profile describing what the Pi can decode (H.265, H.264, common audio). Jellyfin answers with a direct-play URL or a transcode URL that accepts a start position.
- Authentication uses an HTTP header, not a query parameter, so the key stays out of URLs, process arguments and logs.
- surfcult acts as a dedicated Jellyfin user with read access to the libraries it schedules. It does not report playback, so the owner's watched and continue-watching lists stay clean. It stops any transcode it started when a channel changes or playback ends.
- Items are identified by Jellyfin item id, with path as a fallback, and reconciled on each library sync. Whether Tdarr's in-place replacement keeps the item id is unverified (see Risks).
- When Jellyfin is unreachable, local clips and the guide still work, and the screen shows a technical-difficulties slate.

### Player

One MPV process, controlled over its JSON IPC socket. The app owns all scheduling and state, so MPV can be killed and restarted without losing anything. A watchdog restarts it and re-tunes to the current channel. Hardware decoding is requested through MPV and falls back to software.

### TV screen

MPV draws everything. Channel banner, volume, mute and static use MPV's overlay facilities. The guide is rendered by Rust into a bitmap and shown as an MPV overlay, with the video shrunk into a corner, in the style of the Prevue Guide: time header, channel column, a highlighted current cell and an info line. Typography and colour follow the owner's "90s cable" and "analog underground" directions; the visual design is a separate task.

The overlay path is the biggest technical bet in this design, so the first milestone tests it on a Pi.

### Remote control

Key and remote events come in through Linux input devices (keyboard, USB remotes, HDMI-CEC as the kernel exposes it). Keys map to tuner actions: channel up and down, direct number entry, guide, info, mute, volume, back. The key map is configurable.

### Web admin and exports (later)

An Axum server, bound to localhost by default, serves a small admin UI and a REST API over the same TOML and SQLite state. A later phase adds M3U and XMLTV export from the existing schedule, which would also let Jellyfin Live TV or any IPTV client show the guide. Streaming the video itself stays out of scope.

### Security

No telemetry. The admin server is localhost-only unless configured, with optional token authentication. The API never runs shell commands. Clip and config paths are restricted to configured directories. surfcult never deletes media.

## Phases

0. **Hardware spike (throwaway).** On a real Pi: MPV playing a Jellyfin H.265 URL with hardware decoding, a seek to an offset, a preloaded second item, and one overlay frame. Measures tune time, CPU and overlay cost. The result can change this design. Pass thresholds: a direct-play tune completes in 3 seconds or less after the static starts; a transcoded tune is hidden by a tune-in clip of at most 15 seconds; a full-screen guide overlay uses at most about one core and updates smoothly enough to scroll. If the bitmap overlay fails these, the fallback is a text-only guide drawn with MPV's ASS overlays, which gives up the Prevue look but not the function.
1. **Library and timeline.** Jellyfin sync, SQLite, `timeline` and `schedule` with tests, `channels.toml`. Milestone: a command prints what every channel is playing now and for the next day.
2. **Playing.** MPV control, tuner, static, tune-in clips, channel up and down, banner. Milestone: the Pi boots into a TV you can channel-surf.
3. **Television.** Breaks, idents, the guide overlay, remote control, grid scheduling. Milestone: operated entirely by remote.
4. **Admin.** Web admin and API, system health, M3U and XMLTV export.
5. **Later.** Filesystem `MediaSource`, HDMI-CEC polish, weather and emergency channels.

## Testing

- `timeline` and `schedule` are pure and carry most of the tests: position lookup, the look-ahead seek, grid fill, break fill, DST days, determinism for a fixed seed, and regeneration freezing the next 30 minutes.
- Jellyfin and MPV sit behind traits with fakes, so the tuner state machine runs in tests without hardware.
- A few integration tests run against a real MPV and a recorded Jellyfin response.
- Hardware tests on the Pi 4B: hardware decoding, tune time, long-duration playback, restart recovery, network loss.

## Risks to check early

- Hardware H.265 and H.264 decode through MPV on the Pi 4B, including 10-bit.
- Tune time over HTTP seeks and over a Jellyfin transcode.
- CPU cost of bitmap overlays for the guide.
- Whether Jellyfin keeps an item's id when Tdarr replaces its file.
- Reading remote and HDMI-CEC input without a desktop session.

## Owner inputs and the assumptions they led to

Confirmed by the owner: a Raspberry Pi 4B with 2 GB for the Phase 0 spike; a 4K TV; a library that is mostly 1080p (the owner's recollection, not measured).

Assumptions made from that, to be replaced by measurements:

- The device profile sent to Jellyfin allows H.264 and H.265 up to 1080p, 8 and 10-bit SDR, and the audio codecs AAC, MP3, FLAC, Opus, AC3, E-AC3 and DTS. Anything else (4K, HDR or Dolby Vision video; TrueHD or Atmos audio) is handled by a Jellyfin transcode, which the tune-in clip hides. Audio goes out over HDMI, downmixed to stereo by default and configurable.
- Phase 0 starts by querying the Jellyfin library for codec, resolution, bit depth, HDR and audio-codec counts. If more than a small share needs a transcode, the profile and the Phase 0 thresholds are revisited before Phase 1.

## Left to the builder

SQLite crate, async runtime details, module names inside the crate, the exact overlay mechanism for the guide, key-map defaults, logging format, how the service is packaged and started on boot (systemd is expected).
