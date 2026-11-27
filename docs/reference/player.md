# Player

The `Player` trait in `src/player/` is the screen as the tuner and the run loop see it: show static, the stand-by card or a slate, play or queue a local clip, play or queue a program, seek, stop, show or clear an overlay, and show or clear the channel banner. `MpvPlayer` also applies the display filters. `FakePlayer` records these for tests. `MpvPlayer` in `src/player/mpv/` sends them to a real MPV process over its JSON IPC socket.

## Starting MPV

`MpvPlayer::start(MpvConfig, resolver)` starts MPV and connects to its socket. `MpvConfig::new(runtime_dir)` gives the Pi setup.

| Field | Default | Meaning |
| --- | --- | --- |
| `binary` | `mpv` | The executable, found on `PATH`. |
| `runtime_dir` | (required) | A directory only this user can open, mode 700. It holds `mpv.sock`, the overlay bitmaps, the CRT shader and the stand-by card's subtitle files. It is created if missing. An existing directory that others can open is refused. |
| `output_args` | `PI_OUTPUT_ARGS` | `--profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --hwdec=drm,v4l2m2m --fbo-format=rgba8 --force-window=yes --ao=alsa --audio-device=alsa/plughw:CARD=vc4hdmi0,DEV=0` (ADR 0001, decision 2, and the Pi check of 2026-11-24 for `--fbo-format`). Tests use `--vo=null --ao=null`. |
| `static_source` | `STATIC_SOURCE` | What static plays: grey noise generated live by FFmpeg's `lavfi` at 320x180 and 25 fps, scaled up by MPV. The generator never ends, so no file loops. |
| `standby_source` | `STANDBY_SOURCE` | The stand-by card's picture: colour bars matched to the owner's design, a `lavfi` `color` source with one `drawbox` per colour region (`assets/standby/bars.lavfi`), generated live at 640x360 and 30 frames a second and scaled up by MPV on the GPU. The frames give the CRT shader something to move on. No audio. |
| `standby_text` | `STANDBY_TEXT` | The stand-by card's message, `PLEASE STANDBY`. That text, in any case and with surrounding spaces ignored, is drawn from Oswald outlines; any other text is set in DejaVu Sans Bold (see [the stand-by card](#the-stand-by-card)). It is drawn in capitals whatever its case. |
| `filters` | none | Display filters on everything on screen: programs, clips, static and the stand-by card. |
| `standby_filters` | `crt`, `glitch` | Display filters the stand-by card adds while it shows. |
| `glitch_every` | 20 s | The mean time between two flickers of the stand-by card's band. |
| `command_timeout` | 5 s | How long a command waits for MPV's answer. |
| `start_timeout` | 10 s | How long a new MPV may take to open its socket. |

`MpvConfig::with_display(&config.display)` takes `filters`, `standby_filters` and `glitch_every` from the `[display]` table of `channels.toml` (see [Display filters](#display-filters)).

Every MPV also gets `--no-config --idle=yes --input-terminal=no --ytdl=no --keep-open=no --prefetch-playlist=yes --msg-level=all=error --input-ipc-server=<runtime_dir>/mpv.sock`. MPV's errors go to surfcult's stderr. A Unix socket path is limited to 107 bytes, so keep `runtime_dir` short (for example `$XDG_RUNTIME_DIR/surfcult`).

Hardware decoding: `--hwdec=drm,v4l2m2m` asks for the Pi's H.265 decoder, then its H.264 decoder. MPV decodes in software when neither works. `auto-safe` is not used because only this pair was measured on the Pi.

## Commands

| Trait method | MPV command |
| --- | --- |
| `show_static` | `loadfile <static_source> replace` with `loop-file=inf` |
| `show_standby` | writes `standby.ass` to the runtime directory, `set_property glsl-shaders [...]` with the card's shaders if MPV has others, then `loadfile <standby_source> replace` with `loop-file=inf`, `sub-files-append=<runtime_dir>/standby.ass`, `blend-subtitles=yes` and `secondary-sub-ass-override=no` |
| `play_clip(path)` | `loadfile <absolute path> replace` |
| `queue_program(media, start_ms)` | `set_property http-header-fields [...]`, then `loadfile <url> append` with `start=<seconds>` and `sid=no`, so a program never shows a subtitle track from its file |
| `play_program(media, start_ms)` | the same with `loadfile <url> replace` |
| `queue_clip(path)` | `loadfile <absolute path> append` |
| `seek(position_ms)` | `seek <seconds> absolute` |
| `stop` | `stop` |
| `show_overlay(id, overlay)` | writes the BGRA pixels to `overlay-<id>.bgra` in the runtime directory, then `overlay-add <id> <x> <y> <file> 0 bgra <w> <h> <w*4>` |
| `clear_overlay(id)` | `overlay-remove <id>` |
| `show_banner(banner)` | draws the banner with `paint::banner`, runs the CRT pass at `MpvConfig::crt` (Heavy by default), writes it to `overlay-63.bgra` and sends `overlay-add 63 ...` |
| `clear_banner` | `overlay-remove 63`, only when the banner is up |
| `show_overlay_rows(id, overlay, first_row, rows)` | writes the whole bitmap to `overlay-<id>.bgra`, then `overlay-add` with the offset `first_row * width * 4` and the height `rows`, so only those rows show, at the overlay's `y` |
| `scroll_overlay(id, first_row, rows)` | `overlay-add` again with the same file and a new offset, without writing it; an error if the layer shows nothing or the rows are outside the bitmap. The player remembers each layer's place and size for this until it is cleared or MPV restarts |
| `set_video_area(area)` | `set_property` `video-margin-ratio-left` and `video-margin-ratio-bottom`: 0.5 each for `VideoArea::TopRight`, which leaves the top-right quarter for the video while the guide is open, and 0 for `Full`. MPV keeps them across files; a restart puts them back to 0 |

`replace` empties MPV's playlist, so static, the stand-by card, a clip and `play_program` drop anything queued. Those four and `stop` also remove the banner; queueing leaves it up. The banner is a bitmap in overlay layer 63, drawn after the display filters; `show_overlay` and `clear_overlay` refuse that layer. See [run.md](run.md#the-banner). `loop-file=inf` only matters for a configured source that ends; the default sources are `lavfi` generators that never end, so nothing loops and there is no seam. A program is appended behind the clip and opened early by the playlist prefetch. It is not preloaded paused, because one MPV process cannot hold a second file paused (ADR 0001, decision 5).

Overlay layers are numbered 0 to 62 for callers; 63 is the banner's. The pixels are blue, green, red and alpha bytes, with the colour already multiplied by alpha, and there must be exactly `width * height * 4` of them. Overlays stay up when the item on screen changes.

## The stand-by card

The tuner shows the stand-by card when a channel has nothing to play. It copies the owner's reference design: colour bars with a black band across the upper middle and PLEASE STANDBY on it. In the reference the whole picture's red channel is shifted right, so every colour edge has a red or cyan fringe, the band has a red strip on its left edge, and the text is a cyan copy and a red copy 23 pixels to its right that add up to white where they overlap. The bars include those fringes as their own colour regions. The layout is for a 1920x1080 screen and MPV scales it to the real one.

The band and text are an ASS subtitle file, `standby.ass`, that the player writes into the runtime directory and MPV renders through libass, so the card does not need an FFmpeg built with `drawtext`. The file is the subtitle track of the card's playlist entry only (`loadfile` option `sub-files-append`), with `blend-subtitles=yes` so MPV blends it into the picture before user shaders run, and the CRT shader bends and shades the band and text with the bars. (MPV draws its OSD, `osd-overlay` included, after the shaders.) Both options belong to the card's entry, so MPV drops the subtitles and the blending with it: static, a clip or a program never shows them, and no command is needed to remove them. A new MPV after a watchdog restart starts without them, and the tuner shows the card again if it re-tunes to one. Checked with MPV 0.41 here and MPV 0.40.0 on the Pi (`--vo=null`): the option is accepted, the track is selected, and the next `loadfile replace` leaves no subtitle track and `blend-subtitles` off. The man page of MPV 0.40 lists `--glsl-shaders` among what `--blend-subtitles` subjects subtitles to.

The default text is drawn as vector outlines (`\p3` drawings, a quarter-pixel grid) in `assets/standby/text.drawing`. They are derived from the Oswald typeface, weight interpolated between Regular and Bold per letter and fitted to the reference, so the Pi needs no font for them. The white overlap is the red copy clipped to the cyan one (`\clip` with a drawing), which only works with drawings. `tools/standby/gen.py` makes the drawing and the bars from `tools/standby/fit.json`; `tools/standby/README.md` says how to run it. Oswald is licensed under the SIL Open Font License 1.1, and the outlines are derived from it, so the licence and the Oswald copyright line are kept in `tools/standby/OFL.txt`.

A `standby_text` other than `PLEASE STANDBY` cannot use those outlines. It is set in DejaVu Sans Bold narrowed to 80 %, 200 pixels, centred on the band, as a cyan copy 8 pixels left, a red copy 8 pixels right and a white copy on top. A text longer than 14 characters, as many as `PLEASE STANDBY` has, is set smaller in proportion (`TECHNICAL DIFFICULTIES` at 127 pixels) so it stays inside the band; a text with line breaks is counted as one line. The Pi has DejaVu; a system without it gets libass's fallback font. Braces, backslashes and line breaks in the text are escaped, so the text cannot change the drawing or start another event.

`Player::show_slate(slate)` shows the same card with the slate's text: `TECHNICAL DIFFICULTIES` (`Slate::TechnicalDifficulties`) while Jellyfin is unreachable, and `SETTING THE CLOCK` (`Slate::ClockNotSet`) until the clock is trusted ([run.md](run.md#failures)). The glitch redraws whichever text is up, and `show_standby` brings back `standby_text`.

If the card's file cannot be written, or MPV refuses the shaders or the `loadfile`, `MpvPlayer` shows static in its place and returns the error, so the screen never shows bars without their text. The tuner records a `Player` failure and static stays up. MPV loads the subtitle file after `loadfile` returns; a file it cannot read would leave the bars without text, but the player writes it just before and checks that the write succeeded.

The bars are generated at 640x360 and 30 frames a second and scaled up by MPV, so their edges sit on the 640-pixel grid (at most 1.5 pixels off at 1080p) and are slightly soft. FFmpeg 7.1.5 on the Pi gives the same frame as FFmpeg 9 here and runs the graph at about 205 frames a second, about 15 % of one core at 30.

Compared with the reference (612x408, scaled up to 1920x1080 with bicubic), the card rendered by MPV 0.41 `--vo=gpu --profile=fast` without filters differs by a mean absolute error of 2.5 per channel (0 to 255) over the whole frame: 0.9 on the upper bars, 1.7 on the middle strip, 0.5 on the bottom, 9.3 around the band and 13.6 in the text box. Most of what is left is on letter edges: the reference is a JPEG with soft, ringing edges. The previous card (SMPTE HD bars and DejaVu text) differed by 102.

## Display filters

A display filter is a named effect on the picture, `player::Filter`. There are two:

- `crt`, the CRT shader: a GLSL user shader that makes the picture look like a CRT television.
- `glitch`: at random moments the stand-by card flickers for 80 to 150 ms. One or two thin slices of its band move a few pixels sideways and the red and cyan copies of the text move a few pixels further apart, then it settles.

They are set in the optional `[display]` table of `channels.toml`:

```toml
[display]
filters = []                           # on everything on screen
standby_filters = ["crt", "glitch"]    # added while the stand-by card shows
glitch_every = "20s"                   # mean time between flickers
```

Without the table, these defaults apply. An empty list turns its filters off. An unknown filter name or a bad `glitch_every` is a validation problem naming its field (`display.filters[1]`, `display.glitch_every`); an unknown key in the table is a parse error.

While the stand-by card shows, its filters are the global list followed by its own, each filter at most once (`player::standby_filters`). A filter that is on globally is not applied a second time, and the glitch runs at most once whether it is listed globally, for the card, or both. The tuner shows the card on empty channels, gaps (including off-air time after a sign-off), `NoProgramSoon` and programs the media source cannot look up, so the card's filters apply there without any other setting ([tuner.md](tuner.md#when-there-is-nothing-to-show)).

### The CRT shader

The shader is `assets/shaders/crt.glsl`, compiled into the binary (`player::mpv::CRT_SHADER`). MPV loads shaders from files, so `MpvPlayer::start` writes it to `crt.glsl` in the runtime directory and removes it when the player is dropped. It is one pass hooked at `OUTPUT`, after scaling, with two texture reads per pixel:

- scanlines every 3 screen lines, dimming the dark half by up to 10 %;
- a slight barrel curvature (the corners bend out by about 2 %, black outside) and a soft vignette that darkens the corners by about 12 %;
- red and cyan fringing that grows toward the edges, up to 2.5 pixels apart at the corners;
- a faint flicker from MPV's per-frame `random` uniform, up to 1.5 % darker;
- a band 4 % brighter that rolls down the screen once every 420 frames (14 s at 30 frames a second), from MPV's `frame` uniform;
- a gain of 1.08 that makes up for the scanlines and vignette.

The strengths are `#define`s at the top of the file. The reference card is bright and saturated, and the first version of the shader darkened it a lot: on the new card its mean luma (BT.709 weights) was 78 % of the card without the shader. This version keeps 95 to 96 % over the whole frame and about 99 % over the centre half; the rest is mostly the black outside the curved corners.

The player sets MPV's `glsl-shaders` property to the global shaders as soon as MPV is up, and again after a watchdog restart. `show_standby` switches it to the card's shaders, and the first command that leaves the card (`show_static`, `play_clip`, `queue_program` or `stop`) switches it back. The player remembers what MPV has and sends nothing when the list would not change.

The card's band and text go through the shader, because they are subtitles blended into the picture. MPV draws its OSD after the shader, so bitmaps from `show_overlay` stay flat and sharp.

### The glitch

The glitch needs a clock, and the tuner gives the player none, so `MpvPlayer` uses `std::time::Instant` and steps the glitch in `poll_event`. While the card is up it waits from half to one and a half times `glitch_every`, then flickers for 80 to 150 ms. A flicker picks one slice in the upper half of the band, one in the lower half, or both. Each slice is 4 to 13 pixels high (of 1080) and moves 3 to 11 pixels left or right, and the cyan and red copies of the text move 2 to 6 pixels further apart, half each way. The random numbers come from a seeded xorshift generator, so a run can be repeated.

A flicker is a second subtitle file, `standby-flicker.ass`: the whole card drawn again with the slices moved and the copies spread. Each slice is cut out of the letter outlines in the player (curves become short straight segments), so the white overlap still works inside it. MPV shows the file over the card's own track as its secondary subtitle track (`sub-add <file> auto`, then `secondary-sid` set to its id from `track-list`), and when the flicker ends the player sets `secondary-sid` to `no` and removes the track (`sub-remove`). `secondary-sub-ass-override=no` on the card's entry keeps the drawings; MPV's default for secondary subtitles strips ASS styling. Secondary subtitles are blended and shaded like the primary ones. The card's own track stays selected the whole time, so no frame shows the bars without the band. Rewriting the card's file and sending `sub-reload` instead left one or two frames without the band after most reloads in screen captures here (60 captures a second). Because the card stays drawn underneath, a slice that moves away from one end of the band looks stretched at that end rather than moved.

The glitch acts only while the card is up. It stops when anything replaces the card or MPV restarts; a flicker track in progress goes with the card's playlist entry. Its timing is only as fine as the caller's calls to `poll_event`: a flicker lasts until the first poll after its end, so the loop should poll at least every 50 ms while the card shows.

### GPU cost on the Pi

Measured on the Pi 4B on 2026-11-24 (details in `docs/research/2026-11-24-pi-hardware-checks.md`; rerun with the `tests/pi.rs` checks, see `docs/runbooks/raspberry-pi.md`). The card is a 30 frames a second source on a 60 Hz output.

- **The card with the default filters is smooth enough.** MPV uses about 0.5 cores whatever the filters, and the player's own loop (polling every 20 ms for the glitch) about 0.006. With `--fbo-format=rgba8` (now in `PI_OUTPUT_ARGS`) the CRT card drops 7 to 20% of its frames (45 of 600 in 20 s with the `mpv` command line; 56 to 123 with the player and its default filters, the runs varying); with MPV's default 16-bit intermediate format it dropped 20 to 40%, and even an empty user shader dropped 22 to 29%.
- **A global `crt` is not usable on the Pi.** Any user shader makes MPV draw hardware-decoded video through a full-screen GPU pass, and the V3D cannot keep up at 1080p. On a 24 frames a second file it dropped 13% of frames with `rgba8` (37% without) and on 60 frames a second test files 50 to 70%. Hardware decoding stays on (`hwdec-current` is still `drm` or `v4l2m2m`); the drops are the GPU pass. Keep `crt` out of `filters`; it is the default on the card only.
- **`vo=gpu-next` does not help.** It handles the shader better on the card, but with `--hwdec=drm` MPV 0.40 leaks dmabuf file descriptors ("Failed to duplicate dmabuf fd: Too many open files") and stops after a few seconds of video. Do not use it for programs.
- **The look on the TV is the owner's check.** The screenshots the check saves show what MPV drew, not the TV.

### What was checked on the laptop

The shader compiles and runs under OpenGL ES 3.1 (Mesa with `MESA_GLES_VERSION_OVERRIDE=3.1`, the Pi's GLES version) and under Vulkan, with MPV 0.41 `--vo=gpu` in a headless sway. The card with and without the shader, its flicker, and the secondary track going through the shader were checked there too, from screenshots of the real player. The opt-in test `the_crt_shader_compiles_on_a_gpu` repeats the OpenGL ES check when `SURFCULT_GPU_TEST=1` is set. It needs a display: run it inside a desktop session, or start a headless sway (`WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway`) and set `WAYLAND_DISPLAY` to its socket.

## Programs and the Jellyfin key

The player does not know Jellyfin. It asks a `StreamResolver` for a `Stream`: a URL, the HTTP headers it needs, and `stream_offset_ms`, the media position where the stream begins. That offset is 0 for a file or a direct-play stream, and the start position for a transcode that Jellyfin starts there. The player subtracts it from start and seek positions and adds it to the position it reports. A closure `Fn(&MediaId, u64) -> Result<Stream, PlayerError>` is a resolver; the Jellyfin one is issue #21.

The headers, such as `Authorization: MediaBrowser Token="KEY"`, are sent through the socket as MPV's `http-header-fields` property just before the program is appended. They are never in MPV's arguments, the URL or a file. A stream without headers clears the property, so the key is not sent to another server.

`http-header-fields` is one global MPV property, not a setting of one playlist entry. While it is set, MPV sends the headers with every HTTP request it makes, including requests for the item already on screen. Today that is safe because every program comes from Jellyfin and at most one program is queued at a time, and static and clips are not HTTP, so the headers only ever reach the Jellyfin server. A resolver that returns streams from another server, or a change that queues more than one program, must revisit this. `Stream`'s debug output shows header names only, and error messages name the MPV command but not its arguments.

The headers cannot be the playlist entry's own option. With `--prefetch-playlist=yes`, MPV 0.41 opened the prefetched stream without the entry's `http-header-fields`. ADR 0001 suggested a config file passed with `--include`. The property keeps the key off the disk as well as out of the process list.

## Events

| MPV event | `PlayerEvent` |
| --- | --- |
| `end-file` with reason `eof` for a clip | `ClipEnded` |
| the first `playback-restart` of a program | `ProgramStarted { position_ms }`, from `time-pos` plus the stream offset, or the requested start if `time-pos` is unavailable |
| `end-file` with reason `error` for the clip or program on screen | `Failed` |
| (watchdog) | `Restarted` |

`end-file` for static or the stand-by card, with any other reason (`stop`, `redirect`), for an entry that never started, or for an entry a later command replaced, reports nothing. Later `playback-restart` events of a program come from seeks and are dropped.

A `PlayerError` says whether the program source did not answer (`PlayerError::unreachable`, `is_unreachable`): every program would fail the same way until it does. `run::JellyfinResolver` returns one when Jellyfin's `PlaybackInfo` request fails to connect or times out, or answers 502, 503 or 504; any other refusal is about the one item.

## The watchdog

`poll_event` checks MPV each time it is called. At most once every 3 seconds (`LIVENESS_INTERVAL`) it also sends `get_property pid` and waits up to `command_timeout` for the answer, so a hung MPV is found even when the tuner sends nothing. When the process has exited, the socket closed, or a command or that check got no answer within `command_timeout`, the player kills what is left of MPV and starts a new one, at most once a second (`RESTART_DELAY`). When the new MPV is connected, `poll_event` returns `Restarted`. Commands sent while MPV is down return a `PlayerError`.

`poll_event` does this work itself, so it can block:

- The liveness check blocks for up to `command_timeout` (5 s) when MPV does not answer.
- A restart blocks for up to `start_timeout` (10 s) while the new MPV opens its socket. While MPV keeps failing to open it, the attempt repeats about once a second.

`Tuner::pump` calls `poll_event`, so it stalls for as long as these take, and so does whatever loop calls `pump`.

The player keeps no copy of earlier commands to replay. On `Restarted` the tuner tunes the current channel again (static, a tune-in clip, then the program at its live position), because the schedule and the channel are the tuner's. Overlays are lost too; whoever showed them shows them again.

## Tests

`src/player/mpv/tests.rs` tests the protocol against a fake MPV, a Unix socket in a test thread, including the liveness check and a fake that stops answering. The `real` tests run the `mpv` binary headless on clips made by `ffmpeg`: a clip then a queued program at its offset, the key reaching a local HTTP server as a header and not in the URL or MPV's arguments, a killed MPV coming back, a stopped (`SIGSTOP`) MPV being replaced without any command, the stand-by card still playing after 4 seconds and then replaced by a clip, and the card switching MPV's `glsl-shaders` to the CRT shader and back, selecting its subtitle track with blending, showing a flicker as the secondary track and removing it, and leaving no subtitles, blending or secondary track behind when static replaces it. They print a note and pass when `mpv` or `ffmpeg` is missing, as on CI. The fake-MPV tests check the card's exact `loadfile`, that leaving it (also in the middle of a flicker) sends no command of its own, the flicker's commands, the fall back to static when MPV refuses the card or its file cannot be written, the shader switches onto and off the card, the global shaders after a restart (a script stands in for MPV), and the glitch flickering and settling only while the card is up. Unit tests in `src/player/mpv/card.rs` check the card's ASS: the drawn default text, the DejaVu text and its escaping, and a flicker's slices and spread.

Not tested here: the Pi with hardware decoding and the TV, including killing MPV during playback there, the CPU and GPU cost of static, the stand-by card and the CRT shader on the Pi, and how the card and its filters look on the TV.
