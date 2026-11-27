# Running the TV

`surfcult run` is the TV on the Pi: it reads the remote and keyboards, changes channels with the tuner, keeps the screen on each channel's schedule and shows the channel banner. `surfcult now` prints what each channel is showing, to check the TV against, and `surfcult schedule --day` prints each channel's next 24 hours. The code is in `src/run/`, `src/input/` and `src/overlay/`; `src/main.rs` only parses the command line.

## Commands

```bash
surfcult run <channels.toml> <db>
surfcult now <channels.toml> <db>
surfcult schedule --day <channels.toml> <db>
```

`run` opens the database read-write and creates it if needed. `now` opens it read-only, needs no Jellyfin, and prints one line per channel in config order:

```
   3  SATURDAY CARTOONS  The Simpsons: Bart the Genius  19:00-19:30  at 0:12:30
   4  MOVIE NIGHT  off the air until 06:00
   5  ALL DAY CARTOONS  break ad-07.mp4  19:11-19:12  at 0:00:21
   6  NEWS  nothing scheduled
```

A program line has the title (`Series: Episode` for an episode), the local start and end of the whole program, breaks included, and the live position in the media. A break, filler or ident line has the clip's file name, the event's times and the position in the clip. `off the air until` gives the start of the next stored event; `nothing scheduled` means the store has no event for the next seven days. `run` and `now` read the same clock, so `now` run at the moment the TV changes channel shows what the TV should be showing.

`schedule --day` is read-only like `now`: it prints the stored schedule, never a new draw, and needs no Jellyfin. Each channel in the config gets its number and name, then one indented line per program that overlaps the next 24 hours, with the program's local start and end (breaks included, as in the guide) and its title, or `nothing scheduled`:

```
   3  SATURDAY CARTOONS
      19:00-19:30  The Simpsons: Bart the Genius
      19:30-20:00  The Simpsons: Homer's Odyssey
   6  NEWS
      nothing scheduled
```

Every channel in the config is listed; the config has no way to disable one.

## Environment

| Variable | Meaning |
| --- | --- |
| `SURFCULT_JELLYFIN_URL` | Required. The Jellyfin address, such as `http://10.0.0.200:8096`. |
| `SURFCULT_JELLYFIN_API_KEY_FILE` | The file that holds the API key (preferred; mode 600). |
| `SURFCULT_JELLYFIN_API_KEY` | The key itself, when there is no file. |
| `SURFCULT_JELLYFIN_USER_ID` | The Jellyfin user playback is asked for as. Without it, the first enabled user. See [Jellyfin source](jellyfin.md#playback-urls). |
| `SURFCULT_SYNC_MINUTES` | Minutes between two library syncs. Default 15. |
| `SURFCULT_BANNER_SECONDS` | Seconds the banner stays up. Default 5. |
| `SURFCULT_CRT` | The default CRT pass on the interface (the banner, the watermark, the guide and the menu): `heavy` (the default), `light` or `off`. A level the viewer chose in the Picture menu wins over it. See [paint.md](paint.md#the-crt-pass). |
| `SURFCULT_AUDIO_DEVICE` | MPV's audio device. Default `alsa/plughw:CARD=vc4hdmi0,DEV=0`, the first HDMI port. List the devices with `mpv --audio-device=help`; the second port is `alsa/plughw:CARD=vc4hdmi1,DEV=0`. |
| `SURFCULT_CLOCK_TRUSTED` | `1` trusts the clock and `0` distrusts it, overriding systemd. For testing. |
| `XDG_RUNTIME_DIR` | MPV's runtime directory is `$XDG_RUNTIME_DIR/surfcult`, or `/run/user/<uid>/surfcult` without it. |

The key goes only into the `Authorization: MediaBrowser Token="..."` header, for Jellyfin's API and for MPV's `http-header-fields` property. It is never in a URL, a command line or a log line.

## Startup

1. Load `channels.toml` and open the store, migrating it.
2. Scan the clip pools with ffprobe. Skipped files are logged.
3. Start the sync worker. It fetches the library at once and then every `SURFCULT_SYNC_MINUTES`, on its own thread.
4. Start MPV with the Pi output flags and the `[display]` filters. The runtime directory is created with mode 700.
5. If the clock is not trusted, show the clock slate and wait for it (see [the clock](#schedules-and-the-clock)). Otherwise show the stand-by card, extend every channel's schedule and tune the lowest channel number.
6. Loop every 20 ms: write a finished sync into the store, then run one step.

A bad config, a missing Jellyfin address or key, or an MPV that does not start ends `run` with an error. Everything after startup is logged and the loop goes on.

## The loop

One thread owns the store. Each step:

1. reads the keys pressed since the last step;
2. reads the player's events, hands them to the tuner and the driver, and ticks the tuner;
3. when the tuner's sequence has settled, on the program or on the stand-by card, hands the screen to the playback driver and shows the banner;
4. lets the driver move the screen to the next event;
5. clears the banner when its time is up, and shows, fades or hides the SCTV watermark;
6. extends the schedules when that is due.

The player is polled every step, at most 20 ms apart, as the stand-by card's glitch needs. A watchdog restart of MPV blocks a step for up to 10 s; the next step catches up.

## Schedules and the clock

Each channel's sources are resolved into program pools from the library cache (`library::resolve_pool`) and `schedule::extend` fills the channel to seven days ahead. This happens at startup, after every sync that was written, and every hour.

After a sync that changed the library, each channel goes through `schedule::reschedule` instead. A channel whose schedule plays an item that got a new runtime or left the library is regenerated: the next 30 minutes stay and the rest is rebuilt, and each program replaced for an item that left is recorded as a substitution. The log line `library changed: N renamed, N with a new runtime, N gone` reports what the sync found. While the clock is not trusted the changes wait for the first schedule upkeep that runs.

A channel with a source that cannot be resolved is skipped and logged once with the channel and the reason, for example `channel 5: jellyfin_series "Name" matches nothing in the library`. Before the first sync every source fails this way. A skipped channel plays the events it already has and then shows the stand-by card; the other channels are not affected.

Schedules are written only while the clock is trusted, so a Pi that has not synchronized its clock never stores a wrong week. The check, in order:

1. `SURFCULT_CLOCK_TRUSTED` (`1` or `0`);
2. `timedatectl show -p NTPSynchronized --value` printing `yes` (the kernel's synchronized flag, which systemd-timesyncd and chrony both set);
3. if `timedatectl` cannot run, the file `/run/systemd/timesync/synchronized`.

While the clock is not trusted the check repeats every minute.

Until the clock is trusted for the first time the TV shows the clock slate, the stand-by card with `SETTING THE CLOCK` in place of `PLEASE STANDBY`. It tunes nothing, plays no stored event (they would be read at the wrong time) and writes no schedule. Only the quit key works; a restarted MPV gets the slate again. Once a check finds the clock trusted, the TV starts as it would have at startup: schedules are filled and the lowest channel is tuned. A clock that loses its synchronization later does not bring the slate back; schedule upkeep just waits for it.

## Keys

The remote is the Linux input device whose name contains `vc4-hdmi` (the Pi's CEC adapter). It is found by name because its event number changes between boots. Every device with the `kbd` handler is read as a keyboard. Devices are read from `/dev/input/eventN`, which needs the `input` group, and the device list is read again every 3 seconds, so a device plugged in later works. The devices are not grabbed.

Which key does what is the optional `[keys]` table of `channels.toml`. Each key is an action and each value a list of Linux key names (`KEY_UP`, as in `linux/input-event-codes.h`). An action the table names replaces that action's default keys, and a key it gives to an action is taken from any default action that has it. An empty list turns an action off. Names cover letters, digits, the keypad (`KEY_KP0`), `KEY_NUMERIC_0` to `KEY_NUMERIC_9`, the arrows, `KEY_F1` to `KEY_F12` and the media and remote keys below. An unknown action, an unknown key name, or one key under two actions in the table is a validation problem naming its field (`keys.guide[1]`); a value that is not a list of strings is a parse error.

```toml
[keys]
channel_up = ["KEY_UP", "KEY_CHANNELUP", "KEY_PAGEUP"]
digit_5 = ["KEY_5", "KEY_KP5", "KEY_NUMERIC_5"]
```

| Action | Default keys | Does |
| --- | --- | --- |
| `channel_up` | `KEY_UP`, `KEY_CHANNELUP`, `KEY_PAGEUP` | Next channel up, wrapping from the highest number to the lowest |
| `channel_down` | `KEY_DOWN`, `KEY_CHANNELDOWN`, `KEY_PAGEDOWN` | Next channel down, wrapping |
| `digit_0` to `digit_9` | `KEY_0` to `KEY_9`, `KEY_KP0` to `KEY_KP9`, `KEY_NUMERIC_0` to `KEY_NUMERIC_9` | Enters a channel number, see below |
| `info` | `KEY_OK`, `KEY_ENTER`, `KEY_INFO`, `KEY_I` | Shows the banner again |
| `quit` | `KEY_Q` (keyboard only) | Quits |
| `back` | `KEY_EXIT`, `KEY_BACK`, `KEY_ESC`, `KEY_BACKSPACE` | Closes the guide; otherwise nothing yet |
| `menu` | `KEY_MENU`, `KEY_M` | Opens and closes the Picture menu, see below |
| `guide` | `KEY_EPG`, `KEY_PROGRAM`, `KEY_G`, `KEY_PLAYCD`, `KEY_PAUSECD`, `KEY_PLAYPAUSE` | Opens and closes the guide, see below |
| `left` | `KEY_LEFT` | In the guide, half an hour earlier; otherwise nothing |
| `right` | `KEY_RIGHT` | In the guide, half an hour later; otherwise nothing |
| `mute` | `KEY_MUTE` | Nothing yet |
| `volume_up` | `KEY_VOLUMEUP` | Nothing yet |
| `volume_down` | `KEY_VOLUMEDOWN` | Nothing yet |

`mute`, `volume_up` and `volume_down` are read and passed to the loop as `Key` values, which ignores them for now; the volume comes in a later issue. The defaults suit the TCL Fire TV remote over HDMI-CEC: its Menu key is `KEY_MENU`, Back is `KEY_EXIT`, OK is `KEY_OK`, and Play/Pause alternates `KEY_PLAYCD` and `KEY_PAUSECD`, so both open the guide. Its Home, volume and mute keys do not reach the Pi, and it has no number keys. A keyboard's number row and keypad enter channel numbers.

Number entry: a digit starts a channel number, and each digit adds to it. The channel is tuned at once when the number is as wide as the highest channel number (at least two digits), and otherwise 2 seconds after the last digit. A number that is not a channel is ignored. Channel up and down end the entry and change the channel as usual. Other keys leave it alone.

Only a press counts; a held key changes the channel once. Left and right do nothing yet.

SIGINT, SIGTERM and SIGHUP quit too. Quitting stops the transcodes, stops MPV and waits for a library fetch in progress (at most 30 s).

## Changing channel

A channel change stops the transcodes of the program on screen and runs the tuner's sequence: static for 0.5 s, a clip from the tune-in pool, then the program at its live position ([tuner](tuner.md)). The tune-in pool is the top-level `tune_in_pool` key of `channels.toml`, `tune-in` when the key is not set. A pool the key names must exist. When the pool has no clips, a change shows static and then the program, without a clip.

## The playback driver

When the tuner's sequence is over the driver follows the channel's stored events. At each event boundary it shows what the schedule has then:

- **Program**: its media at the live position. A program cut by breaks continues at the next segment's offset. A segment that continues the media on screen with no break between needs no command.
- **Break, filler or ident**: the local clip whose canonical path is the event's media id. A clip shorter than its event, such as a break's pad that repeats one ident, loops until the event ends. A clip that is not in the scanned pools, or that the player cannot play, shows the stand-by card for the rest of the event.
- **No event** (a gap, dead air, or off the air after a sign-off): the stand-by card until the next event. A program after the card starts its expected start-up time early (1 s for direct play, 7 s for a transcode), so it is on screen when its event starts; a clip after the card starts on time.

The position always comes from the schedule at the current time. Switching away and back never restarts a program, and never resumes where the viewer left.

How each move is made:

- **After a clip: queued.** A clip's length is known exactly, so when a clip ends at its event's end, the next item is queued behind it as soon as the clip starts. MPV opens it while the clip plays and starts it when the clip ends. A break of several clips and the program after it play without a gap. A queued clip shorter than its event is queued to loop. A looping clip has nothing queued behind it; what follows replaces it at the boundary.
- **After a program: replaced at the boundary.** A segment cut by a break never ends where its file does, and a program's real length is not known until the player opens it. Replacing a program with a clip or the card is immediate. Replacing it with another program, on a channel without breaks, shows nothing for the new program's start-up time.
- If the loop was late (a watchdog restart) and the queued item is no longer what is on, what is on now replaces it.

Late starts follow the tuner's rule: a program the driver started that reports its start more than 500 ms behind its live position gets one seek to it. A program that starts early is not corrected.

A program's file may be shorter or longer than its event. When the player reports the file's real length, the driver fits it (`schedule::fit`): a shorter file is padded from the channel's filler pool up to the event's end, and a longer one is cut there. Each mismatch is written to the playback history as `padded`, `short` (no filler clip) or `cut` (`schedule::record_fit`).

After a tune that ended without a program, on the stand-by card, the driver takes over at once: whatever is on now plays without a tune-in clip, or the card stays until the next event. A long filler stretch the tuner did not play this way plays from its next clip.

## Transcodes

The stream resolver asks Jellyfin how to play each program (`JellyfinClient::playback`). A direct-play URL begins at the start of the media and MPV seeks to the position; a transcode begins at the position. Each transcode session is remembered and stopped (`JellyfinClient::stop_transcode`) when the program leaves the screen: a channel change, a boundary that replaces it, a player restart, and quitting. Stopping is best effort; an error is logged.

## The banner

The banner shows when a channel change has settled, on the program or on the stand-by card, when the player restarted and the tuner tuned again, and on OK. It stays up for `SURFCULT_BANNER_SECONDS` (5 s). It has three lines on a dark translucent box across the bottom of the screen:

1. the channel number and name;
2. the program on now (`Series: Episode` for an episode). A break inside the program still names the program. A break, filler or ident between programs shows `NEXT:` and the next program. A gap shows `OFF THE AIR`, and a channel with nothing stored for seven days `NO SCHEDULE`;
3. the program's local start and end (`19:00–19:30`, breaks included), or `BACK AT 06:00` when off the air.

The player draws it in the Cable Grid design ([paint.md](paint.md#the-banner)): a channel cell with the number and name, an ochre strip with the times over the title, and a clock cell with the local time it was shown. It gets the CRT pass at `SURFCULT_CRT` and is shown in overlay layer 63, which the player keeps for it. MPV draws overlays after its shaders, so the CRT shader does not touch it. Any command that replaces the screen or stops it removes the banner, so a boundary inside the 5 seconds takes it away early. A restarted MPV has no banner until the tuner's new sequence settles.

## The guide

The guide key opens the program guide on the channel being watched ([paint.md](paint.md#the-guide)). The video moves into the top-right quarter (`Player::set_video_area`), the banner goes, and the watermark hides until the guide closes. While it is open:

- `channel_up` and `channel_down` move the focus up and down the channel rows, stopping at the first and last channel. They do not change channel. Four rows are on screen and they follow the focus.
- `left` and `right` move the grid half an hour earlier or later, from the current half hour up to 24 hours ahead (`run::guide::MAX_LATER`). The focused cell is then the program on at the grid's first slot, and the body and rows are drawn again.
- `info` (OK) tunes the focused channel and closes the guide; on the channel being watched it only closes it.
- `guide` and `back` close it without tuning.
- A digit closes it and starts a channel number as usual.

The grid covers the current half hour and the two after it. It is three overlays, each given the CRT pass at `SURFCULT_CRT`:

| Layer | What | Drawn |
| --- | --- | --- |
| 1, `run::GUIDE_LAYER` | everything but the channel rows and the clock | when the guide opens, when the focus moves, and when a new half hour begins |
| 2, `GUIDE_CLOCK_LAYER` | the clock cell | each second |
| 3, `GUIDE_ROWS_LAYER` | every channel's row in one tall bitmap, with the first four again after the last | with layer 1; four rows of it show at a time (`Player::show_overlay_rows`) |

After 5 seconds without a key (`run::guide::IDLE_MS`), the rows scroll upward on their own: every 100 ms the player shows the bitmap from 4 pixel rows further down (`Player::scroll_overlay`), without drawing or sending it again, and after the last channel the first comes round again. Rows are 92 pixels apart and steps are 4, so the scanlines stay on the screen's rows. The next key only stops the scroll and puts the rows back where the focus is; the wait starts again from that key. Four channels or fewer fit on screen and never scroll.

Closing the guide, changing channel or a player restart removes all three layers and gives the video the whole screen again.

## The Picture menu

The menu key opens the Picture menu over the whole screen, in overlay layer 4 (`run::MENU_LAYER`), with the CRT pass at the current level ([paint.md](paint.md#the-menu)). The guide closes, the banner goes, and the watermark hides while it is open. Its one setting is the CRT filter:

- `info` (OK) steps it to the next level, Heavy, Light, Off and round again. The new level applies at once to the menu, the watermark, the guide and the banner (`Player::set_crt`), and is stored in the store's viewer settings (`viewer_settings`, name `crt`), so it outlives a restart. At start a stored level wins over `SURFCULT_CRT`; a stored value that does not parse is logged and ignored.
- `channel_up` and `channel_down` do nothing, as there is one setting.
- `menu` and `back` close it. The guide key and a digit close it and then do what they usually do.

A channel change or a player restart closes it.

## The watermark

The SCTV watermark is the station bug in the top-right corner, drawn in overlay layer 8 (`run::WATERMARK_LAYER`) at 60% opacity, with the CRT pass at `SURFCULT_CRT`. Its look and place are in [paint.md](paint.md#the-watermark).

- When a channel change settles, the moment the banner shows, the beside form (the mark with SCTV beside it) appears, holds for 3 seconds and fades out over 1 second.
- While a break or filler event is on screen, the under form (SCTV under the mark) shows instead, for the whole event. When the break ends it fades out over 1 second. A break cancels the landing's hold and fade.
- Fades move in eighths of the opacity, so a fade is at most eight redraws of the watermark's small bitmap. The loop sends a command only when the form or opacity changes.
- A channel change hides it at once, and a player restart forgets it; it comes back with the next landing.

Each form is drawn and given the CRT pass once, on first use, and then only scaled for each step of a fade.

## Playback history

Each program and clip the driver starts gets a `playback_history` row with the outcome `played`, or `failed` when the player or Jellyfin refused it or the player reported it failed. The tuner's program is recorded as `played` when it starts. Tune-in clips are not recorded.

## Failures

No error stops the loop after startup:

- **An unplayable program.** Jellyfin refuses the item (an error status other than 502 to 504, or no way to play it), or MPV reports that the file or stream would not open or broke off (`end-file` with reason `error`). A program MPV reports failed is first tried once more at its live position, in case the stream only broke off. Then the driver gives up on that event: it logs `cannot play ID: ...; filler takes its place`, records it as `failed`, and fills the rest of the event from the channel's filler pool, the clips in pool order as `schedule::fit` pads a short file. The next event starts on time. A substitution row records the event's item and start, with the first filler clip as the replacement. A channel with no filler pool shows the stand-by card instead, and the substitution has no replacement. The stored schedule is not changed. The driver remembers the event for the rest of the run, so reading the schedule again does not retry it; the same item at another time is tried as usual.
- **Jellyfin unreachable.** When Jellyfin does not answer a program's `PlaybackInfo` request (a connection that fails or times out, or a 502, 503 or 504), the program's time shows the technical-difficulties slate, the stand-by card with `TECHNICAL DIFFICULTIES`. Break, filler and ident clips are local files and still play at their boundaries. The program is tried again every minute while its event lasts and at each later program boundary, and plays at its live position once Jellyfin answers. A retry that fails again changes nothing on screen and is not recorded again. Each attempt waits for the request's timeout (5 s to connect), and the loop does nothing else meanwhile.
- A tune whose program the player refuses leaves the card up, and the driver tries the program once more without a tune-in clip. A tune-in clip or program that MPV reports failed ends the tune the same way.
- A failed library sync is logged and the cache keeps the last good library.
- A schedule that cannot be read is logged and read again a minute later.
- MPV that dies or hangs is restarted by the player's watchdog, and the tuner tunes the channel again.

A program that never starts and never fails, such as a transcode that hangs, keeps the screen as it is until the next boundary.

## Tests

`src/run/tests.rs` drives the loop with a virtual clock, a `FakePlayer` wrapped so single programs can fail as a Jellyfin error would, or all of them as a Jellyfin outage would, a `FakeInput`, an in-memory store whose library cache is the media source, and clip pools scanned from a temporary directory with a fake prober. They cover wrapping channel changes, the commands of a tune, returning to a channel at its live position, a program cut by breaks, a stale queue, gaps and off-air time, the banner and its timeout, an unplayable program with and without a filler pool, a program that fails twice on screen, a Jellyfin outage with a break in it and its recovery, a refused tune, a player restart, an unresolvable source at startup, the clock slate until the clock is trusted, the watermark's hold, fade, break form and channel change, and the guide: opening it, the clock's own layer, moving the focus, tuning with OK, closing with Back, the banner going, the watermark hiding, `left` and `right` moving the grid through time and doing nothing outside it, and the rows shown from their tall bitmap (three channels, which never scroll); and the Picture menu: opening it, OK stepping and storing the CRT filter, a stored level at start, the guide key leaving it, and the watermark hiding. The scroll itself, its loop and a key stopping it are tested in `src/run/guide.rs`. The existing tests run with the watermark off (`Settings::watermark`) so their command lists stay exact; `Rig::watermarked` turns it on. `src/input/evdev.rs` tests parsing `input_event` bytes and `/proc/bus/input/devices`, and `src/input/keymap.rs` the key names, the defaults and the `[keys]` overrides. The loop tests also cover number entry: one digit after the wait, two digits at once, an unknown number and a channel key during entry. The stream resolver is tested against recorded Jellyfin responses. A real MPV test (skipped without `mpv`) checks that MPV accepts `play_program`, `queue_clip` and the banner's overlay.

Not tested here: the remote, the TV, Jellyfin, the look of the banner and the slates on the Pi, and what MPV reports for a missing file or a Jellyfin that goes away mid-program. That is the owner's check: change channels across three channels and compare each with `surfcult now`.
