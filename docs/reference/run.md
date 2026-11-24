# Running the TV

`surfcult run` is the TV on the Pi: it reads the remote and keyboards, changes channels with the tuner, keeps the screen on each channel's schedule and shows the channel banner. `surfcult now` prints what each channel is showing, to check the TV against. The code is in `src/run/`, `src/input/` and `src/overlay/`; `src/main.rs` only parses the command line.

## Commands

```bash
surfcult run <channels.toml> <db>
surfcult now <channels.toml> <db>
```

`run` opens the database read-write and creates it if needed. `now` opens it read-only, needs no Jellyfin, and prints one line per channel in config order:

```
   3  SATURDAY CARTOONS  The Simpsons: Bart the Genius  19:00-19:30  at 0:12:30
   4  MOVIE NIGHT  off the air until 06:00
   5  ALL DAY CARTOONS  break ad-07.mp4  19:11-19:12  at 0:00:21
   6  NEWS  nothing scheduled
```

A program line has the title (`Series: Episode` for an episode), the local start and end of the whole program, breaks included, and the live position in the media. A break, filler or ident line has the clip's file name, the event's times and the position in the clip. `off the air until` gives the start of the next stored event; `nothing scheduled` means the store has no event for the next seven days. `run` and `now` read the same clock, so `now` run at the moment the TV changes channel shows what the TV should be showing.

## Environment

| Variable | Meaning |
| --- | --- |
| `SURFCULT_JELLYFIN_URL` | Required. The Jellyfin address, such as `http://10.0.0.200:8096`. |
| `SURFCULT_JELLYFIN_API_KEY_FILE` | The file that holds the API key (preferred; mode 600). |
| `SURFCULT_JELLYFIN_API_KEY` | The key itself, when there is no file. |
| `SURFCULT_JELLYFIN_USER_ID` | The Jellyfin user playback is asked for as. Without it, the first enabled user. See [Jellyfin source](jellyfin.md#playback-urls). |
| `SURFCULT_SYNC_MINUTES` | Minutes between two library syncs. Default 15. |
| `SURFCULT_BANNER_SECONDS` | Seconds the banner stays up. Default 5. |
| `SURFCULT_CLOCK_TRUSTED` | `1` trusts the clock and `0` distrusts it, overriding systemd. For testing. |
| `XDG_RUNTIME_DIR` | MPV's runtime directory is `$XDG_RUNTIME_DIR/surfcult`, or `/run/user/<uid>/surfcult` without it. |

The key goes only into the `Authorization: MediaBrowser Token="..."` header, for Jellyfin's API and for MPV's `http-header-fields` property. It is never in a URL, a command line or a log line.

## Startup

1. Load `channels.toml` and open the store, migrating it.
2. Scan the clip pools with ffprobe. Skipped files are logged.
3. Start the sync worker. It fetches the library at once and then every `SURFCULT_SYNC_MINUTES`, on its own thread.
4. Start MPV with the Pi output flags and the `[display]` filters. The runtime directory is created with mode 700.
5. Show the stand-by card, extend every channel's schedule (when the clock is trusted) and tune the lowest channel number.
6. Loop every 20 ms: write a finished sync into the store, then run one step.

A bad config, a missing Jellyfin address or key, or an MPV that does not start ends `run` with an error. Everything after startup is logged and the loop goes on.

## The loop

One thread owns the store. Each step:

1. reads the keys pressed since the last step;
2. reads the player's events, hands them to the tuner and the driver, and ticks the tuner;
3. when the tuner's sequence has settled, on the program or on the stand-by card, hands the screen to the playback driver and shows the banner;
4. lets the driver move the screen to the next event;
5. clears the banner when its time is up;
6. extends the schedules when that is due.

The player is polled every step, at most 20 ms apart, as the stand-by card's glitch needs. A watchdog restart of MPV blocks a step for up to 10 s; the next step catches up.

## Schedules and the clock

Each channel's sources are resolved into program pools from the library cache (`library::resolve_pool`) and `schedule::extend` fills the channel to seven days ahead. This happens at startup, after every sync that was written, and every hour.

A sync that changed the runtime of an item in a channel's pools runs `schedule::regenerate` for that channel instead. It keeps the next 30 minutes and rebuilds the rest.

A channel with a source that cannot be resolved is skipped and logged once with the channel and the reason, for example `channel 5: jellyfin_series "Name" matches nothing in the library`. Before the first sync every source fails this way. A skipped channel plays the events it already has and then shows the stand-by card; the other channels are not affected.

Schedules are written only while the clock is trusted, so a Pi that has not synchronized its clock never stores a wrong week. The check, in order:

1. `SURFCULT_CLOCK_TRUSTED` (`1` or `0`);
2. `timedatectl show -p NTPSynchronized --value` printing `yes` (the kernel's synchronized flag, which systemd-timesyncd and chrony both set);
3. if `timedatectl` cannot run, the file `/run/systemd/timesync/synchronized`.

While the clock is not trusted the check repeats every minute.

## Keys

The remote is the Linux input device whose name contains `vc4-hdmi` (the Pi's CEC adapter). It is found by name because its event number changes between boots. Every device with the `kbd` handler is read as a keyboard. Devices are read from `/dev/input/eventN`, which needs the `input` group, and the device list is read again every 3 seconds, so a device plugged in later works. The devices are not grabbed.

| Key | Does |
| --- | --- |
| Up, Channel up | Next channel up, wrapping from the highest number to the lowest |
| Down, Channel down | Next channel down, wrapping |
| OK, Enter, I | Shows the banner again |
| Q (keyboard only) | Quits |

Only a press counts; a held key changes the channel once. Left and right do nothing yet.

SIGINT, SIGTERM and SIGHUP quit too. Quitting stops the transcodes, stops MPV and waits for a library fetch in progress (at most 30 s).

## Changing channel

A channel change stops the transcodes of the program on screen and runs the tuner's sequence: static for 0.5 s, a clip from the tune-in pool, then the program at its live position ([tuner](tuner.md)). The tune-in pool is the top-level `tune_in_pool` key of `channels.toml`, `tune-in` when the key is not set. A pool the key names must exist. When the pool has no clips, a change shows static and then the program, without a clip.

## The playback driver

When the tuner's sequence is over the driver follows the channel's stored events. At each event boundary it shows what the schedule has then:

- **Program**: its media at the live position. A program cut by breaks continues at the next segment's offset. A segment that continues the media on screen with no break between needs no command.
- **Break, filler or ident**: the local clip whose canonical path is the event's media id. A clip that is not in the scanned pools shows the stand-by card for the event.
- **No event** (a gap, dead air, or off the air after a sign-off): the stand-by card until the next event. A program after the card starts its expected start-up time early (1 s for direct play, 7 s for a transcode), so it is on screen when its event starts; a clip after the card starts on time.

The position always comes from the schedule at the current time. Switching away and back never restarts a program, and never resumes where the viewer left.

How each move is made:

- **After a clip: queued.** A clip's length is known exactly, so when a clip ends at its event's end, the next item is queued behind it as soon as the clip starts. MPV opens it while the clip plays and starts it when the clip ends. A break of several clips and the program after it play without a gap.
- **After a program: replaced at the boundary.** A segment cut by a break never ends where its file does, and a program's real length is not known until the player opens it. Replacing a program with a clip or the card is immediate. Replacing it with another program, on a channel without breaks, shows nothing for the new program's start-up time.
- If the loop was late (a watchdog restart) and the queued item is no longer what is on, what is on now replaces it.

Late starts follow the tuner's rule: a program the driver started that reports its start more than 500 ms behind its live position gets one seek to it. A program that starts early is not corrected.

A program's file may be shorter or longer than its event. A shorter file leaves the screen black until the boundary; a longer one is cut there. Padding a short file with filler at play time (`schedule::fit` and `schedule::record_fit`) needs the file's real length from the player and is later work.

After a tune that ended without a program, on the stand-by card, the driver takes over at once: whatever is on now plays without a tune-in clip, or the card stays until the next event. A long filler stretch the tuner did not play this way plays from its next clip.

## Transcodes

The stream resolver asks Jellyfin how to play each program (`JellyfinClient::playback`). A direct-play URL begins at the start of the media and MPV seeks to the position; a transcode begins at the position. Each transcode session is remembered and stopped (`JellyfinClient::stop_transcode`) when the program leaves the screen: a channel change, a boundary that replaces it, a player restart, and quitting. Stopping is best effort; an error is logged.

## The banner

The banner shows when a channel change has settled, on the program or on the stand-by card, when the player restarted and the tuner tuned again, and on OK. It stays up for `SURFCULT_BANNER_SECONDS` (5 s). It has three lines on a dark translucent box across the bottom of the screen:

1. the channel number and name;
2. the program on now (`Series: Episode` for an episode). A break inside the program still names the program. A break, filler or ident between programs shows `NEXT:` and the next program. A gap shows `OFF THE AIR`, and a channel with nothing stored for seven days `NO SCHEDULE`;
3. the program's local start and end (`19:00–19:30`, breaks included), or `BACK AT 06:00` when off the air.

MPV draws it as ASS text through `osd-overlay` (id 1) on a 1920x1080 layout, in DejaVu Sans, white with a thin black border. It is OSD, so the CRT shader does not touch it. Any command that replaces the screen or stops it removes the banner, so a boundary inside the 5 seconds takes it away early. A restarted MPV has no banner until the tuner's new sequence settles.

## Playback history

Each program and clip the driver starts gets a `playback_history` row with the outcome `played`, or `failed` when the player or Jellyfin refused it. The tuner's program is recorded as `played` when it starts. Tune-in clips are not recorded.

## Failures

No error stops the loop after startup:

- A player command or a Jellyfin playback error is logged (`surfcult: channel N: ...`), the stand-by card shows, and the next boundary or key press goes on as usual.
- A tune whose program the player refuses leaves the card up, and the driver tries the program once more without a tune-in clip.
- A failed library sync is logged and the cache keeps the last good library.
- A schedule that cannot be read is logged and read again a minute later.
- MPV that dies or hangs is restarted by the player's watchdog, and the tuner tunes the channel again.

Giving up on a program that never starts, and slates for long failures, are issue #34.

## Tests

`src/run/tests.rs` drives the loop with a virtual clock, a `FakePlayer` wrapped so single programs can fail as a Jellyfin error would, a `FakeInput`, an in-memory store whose library cache is the media source, and clip pools scanned from a temporary directory with a fake prober. They cover wrapping channel changes, the commands of a tune, returning to a channel at its live position, a program cut by breaks, a stale queue, gaps and off-air time, the banner and its timeout, a Jellyfin error, a refused tune, a player restart, an unresolvable source at startup and a schedule written once the clock is trusted. `src/input/evdev.rs` tests parsing `input_event` bytes and `/proc/bus/input/devices`. The stream resolver is tested against recorded Jellyfin responses. A real MPV test (skipped without `mpv`) checks that MPV accepts `play_program`, `queue_clip` and the banner's `osd-overlay`.

Not tested here: the remote, the TV, Jellyfin and the look of the banner on the Pi. That is the owner's check: change channels across three channels and compare each with `surfcult now`.
