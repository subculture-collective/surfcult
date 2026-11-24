# Player

The `Player` trait in `src/player/` is the screen as the tuner sees it: show static or the stand-by card, play a local clip, queue a program, seek, stop, and show or clear an overlay. `MpvPlayer` also applies the display filters. `FakePlayer` records these for tests. `MpvPlayer` in `src/player/mpv/` sends them to a real MPV process over its JSON IPC socket.

## Starting MPV

`MpvPlayer::start(MpvConfig, resolver)` starts MPV and connects to its socket. `MpvConfig::new(runtime_dir)` gives the Pi setup.

| Field | Default | Meaning |
| --- | --- | --- |
| `binary` | `mpv` | The executable, found on `PATH`. |
| `runtime_dir` | (required) | A directory only this user can open, mode 700. It holds `mpv.sock` and the overlay bitmaps. It is created if missing. An existing directory that others can open is refused. |
| `output_args` | `PI_OUTPUT_ARGS` | `--profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --hwdec=drm,v4l2m2m --force-window=yes` (ADR 0001, decision 2). Tests use `--vo=null --ao=null`. |
| `static_source` | `STATIC_SOURCE` | What static plays: grey noise generated live by FFmpeg's `lavfi` at 320x180 and 25 fps, scaled up by MPV. The generator never ends, so no file loops. |
| `standby_source` | `STANDBY_SOURCE` | The stand-by card's picture: `av://lavfi:smptehdbars=s=640x360:r=30`, SMPTE HD colour bars generated live at 640x360 and 30 frames a second and scaled up by MPV on the GPU. The frames give the CRT shader something to move on. No audio. |
| `standby_text` | `STANDBY_TEXT` | The stand-by card's message, `PLEASE STAND BY`. It is drawn in capitals whatever its case. |
| `filters` | none | Display filters on everything on screen: programs, clips, static and the stand-by card. |
| `standby_filters` | `crt`, `glitch` | Display filters the stand-by card adds while it shows. |
| `glitch_every` | 20 s | The mean time between two glitches of the stand-by card's text. |
| `command_timeout` | 5 s | How long a command waits for MPV's answer. |
| `start_timeout` | 10 s | How long a new MPV may take to open its socket. |

`MpvConfig::with_display(&config.display)` takes `filters`, `standby_filters` and `glitch_every` from the `[display]` table of `channels.toml` (see [Display filters](#display-filters)).

Every MPV also gets `--no-config --idle=yes --input-terminal=no --ytdl=no --keep-open=no --prefetch-playlist=yes --msg-level=all=error --input-ipc-server=<runtime_dir>/mpv.sock`. MPV's errors go to surfcult's stderr. A Unix socket path is limited to 107 bytes, so keep `runtime_dir` short (for example `$XDG_RUNTIME_DIR/surfcult`).

Hardware decoding: `--hwdec=drm,v4l2m2m` asks for the Pi's H.265 decoder, then its H.264 decoder. MPV decodes in software when neither works. `auto-safe` is not used because only this pair was measured on the Pi.

## Commands

| Trait method | MPV command |
| --- | --- |
| `show_static` | `loadfile <static_source> replace` with `loop-file=inf` |
| `show_standby` | `set_property glsl-shaders [...]` with the card's shaders if MPV has others, `loadfile <standby_source> replace` with `loop-file=inf`, then `osd-overlay` id 1 with the card's band and text in ASS |
| `play_clip(path)` | `loadfile <absolute path> replace` |
| `queue_program(media, start_ms)` | `set_property http-header-fields [...]`, then `loadfile <url> append` with `start=<seconds>` |
| `seek(position_ms)` | `seek <seconds> absolute` |
| `stop` | `stop` |
| `show_overlay(id, overlay)` | writes the BGRA pixels to `overlay-<id>.bgra` in the runtime directory, then `overlay-add <id> <x> <y> <file> 0 bgra <w> <h> <w*4>` |
| `clear_overlay(id)` | `overlay-remove <id>` |

`replace` empties MPV's playlist, so static, the stand-by card and a clip drop anything queued. `loop-file=inf` only matters for a configured source that ends; the default sources are `lavfi` generators that never end, so nothing loops and there is no seam. A program is appended behind the clip and opened early by the playlist prefetch. It is not preloaded paused, because one MPV process cannot hold a second file paused (ADR 0001, decision 5).

Overlay layers are numbered 0 to 63. The pixels are blue, green, red and alpha bytes, with the colour already multiplied by alpha, and there must be exactly `width * height * 4` of them. Overlays stay up when the item on screen changes.

## The stand-by card

The tuner shows the stand-by card when a channel has nothing to play. It is the colour bars from `standby_source` with a black band across the middle and `standby_text` on it in white bold condensed capitals. The text is drawn three times for a chromatic offset: red 8 pixels to the left, cyan 8 pixels to the right, and white on top. The layout is for a 1920x1080 screen (band from 430 to 650 pixels down, text 150 pixels high in DejaVu Sans Bold narrowed to 80 %) and MPV scales it to the real one.

MPV draws the band and text itself, as an `osd-overlay` (id 1, format `ass-events`) rendered by libass, so the card does not need an FFmpeg built with `drawtext`. Braces, backslashes and line breaks in the text are escaped, so the text cannot change the drawing. `osd-overlay` ids are separate from the `overlay-add` layers that `show_overlay` uses. Before `show_static`, `play_clip`, `queue_program` or `stop` sends its own command, the player removes the overlay (`osd-overlay` id 1 with format `none`; MPV refuses it without a `data` argument).

If MPV accepts the bars but refuses the shaders or the overlay, `MpvPlayer` shows static in their place and returns the error, so the screen never shows bars without their text. The tuner records a `Player` failure and static stays up.

The bars are generated at 640x360 and 30 frames a second and scaled up by MPV. Seen at 1920x1080 they look like bars generated at full size, with slightly soft edges between the colours. The first version generated them at 1920x1080 and one frame a second; on this development laptop (Intel i7-12700H, `--vo=null`) that used about 0.1 s of CPU in 10 s of the card, against about 2.5 s for static. The 30-frame source has not been measured on the laptop or the Pi.

## Display filters

A display filter is a named effect on the picture, `player::Filter`. There are two:

- `crt`, the CRT shader: a GLSL user shader that makes the picture look like a CRT television.
- `glitch`, the text glitch: at random moments the stand-by card's band tears sideways and its red and cyan text jump apart, then it settles.

They are set in the optional `[display]` table of `channels.toml`:

```toml
[display]
filters = []                           # on everything on screen
standby_filters = ["crt", "glitch"]    # added while the stand-by card shows
glitch_every = "20s"                   # mean time between glitches
```

Without the table, these defaults apply. An empty list turns its filters off. An unknown filter name or a bad `glitch_every` is a validation problem naming its field (`display.filters[1]`, `display.glitch_every`); an unknown key in the table is a parse error.

While the stand-by card shows, its filters are the global list followed by its own, each filter at most once (`player::standby_filters`). A filter that is on globally is not applied a second time, and the glitch runs at most once whether it is listed globally, for the card, or both. The tuner shows the card on empty channels, gaps (including off-air time after a sign-off), `NoProgramSoon` and programs the media source cannot look up, so the card's filters apply there without any other setting ([tuner.md](tuner.md#when-there-is-nothing-to-show)).

### The CRT shader

The shader is `assets/shaders/crt.glsl`, compiled into the binary (`player::mpv::CRT_SHADER`). MPV loads shaders from files, so `MpvPlayer::start` writes it to `crt.glsl` in the runtime directory and removes it when the player is dropped. It is one pass hooked at `OUTPUT`, after scaling, with two texture reads per pixel:

- scanlines every 3 screen lines, dimming the dark half by up to 16 %;
- a slight barrel curvature (the corners bend out by about 3.5 %, black outside) and a vignette that darkens the corners by about a third;
- red and cyan fringing that grows toward the edges, up to 2.5 pixels apart at the corners;
- a faint flicker from MPV's per-frame `random` uniform, up to 2 % darker;
- a band 5 % brighter that rolls down the screen once every 420 frames (14 s at 30 frames a second), from MPV's `frame` uniform.

The strengths are `#define`s at the top of the file.

The player sets MPV's `glsl-shaders` property to the global shaders as soon as MPV is up, and again after a watchdog restart. `show_standby` switches it to the card's shaders, and the first command that leaves the card (`show_static`, `play_clip`, `queue_program` or `stop`) switches it back. The player remembers what MPV has and sends nothing when the list would not change.

MPV draws its OSD after the shader, so the CRT shader does not change the stand-by card's band and text or bitmaps from `show_overlay`: they stay flat and sharp over the curved bars. MPV does put subtitles through the shader with `--blend-subtitles=yes` (checked with MPV 0.41), so drawing the card's text as a subtitle track would bring it under the shader.

### The text glitch

The glitch needs a clock, and the tuner gives the player none, so `MpvPlayer` uses `std::time::Instant` and steps the glitch in `poll_event`. While the card is up it waits from half to one and a half times `glitch_every`, redraws the card's `osd-overlay` torn for 100 to 200 ms, then redraws it as it was. A torn card cuts the band into four strips. Each strip moves sideways by up to 90 pixels of a 1920x1080 screen and shows only its own rows of the text (`\clip`), and the red and cyan copies sit 18 to 47 pixels from the white one instead of 8. The random numbers come from a seeded xorshift generator, so a run can be repeated.

The glitch acts only while the card is up. It stops when anything replaces the card or MPV restarts. Its timing is only as fine as the caller's calls to `poll_event`: a glitch lasts until the first poll after its end, so the loop should poll at least every 50 ms while the card shows.

### GPU cost on the Pi

Not measured yet; this is the part of issue #72 left to check on the Pi. The shader is one cheap pass at 1920x1080, but the Pi's V3D GPU is small, and ADR 0001 found that MPV's default video settings drop frames there without `--profile=fast`. Check:

1. The card with the default filters: `mpv --profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --glsl-shaders=crt.glsl 'av://lavfi:smptehdbars=s=640x360:r=30'`. Read `frame-drop-count` and `vo-delayed-frame-count` over IPC, and the CPU in `top`.
2. A program with `crt` as a global filter: the same flags plus `--hwdec=drm,v4l2m2m`, on a 1080p H.265 file and a 1080p H.264 file. Check that `hwdec-current` still says `drm` or `v4l2m2m` and that no frames are dropped. To apply a shader, MPV must draw the hardware-decoded frames through the GPU rather than hand them straight to the display. If it cannot import them, it decodes in software or needs a copy path (`--hwdec=drm-copy`), and both cost CPU.
3. How the scanlines, curvature and glitch look on the TV.

If the global shader costs too much, keep it on the card only, which is the default.

### What was checked on the laptop

The shader compiles and runs under OpenGL ES 3.1 (Mesa with `MESA_GLES_VERSION_OVERRIDE=3.1`, the Pi's GLES version) and under Vulkan, with MPV 0.41 `--vo=gpu` in a headless sway. The opt-in test `the_crt_shader_compiles_on_a_gpu` repeats the OpenGL ES check when `SURFCULT_GPU_TEST=1` is set. It needs a display: run it inside a desktop session, or start a headless sway (`WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway`) and set `WAYLAND_DISPLAY` to its socket.

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
| (watchdog) | `Restarted` |

`end-file` for static, the stand-by card or a program, with any other reason (`stop`, `redirect`, `error`), or for an entry a later command replaced, reports nothing. Later `playback-restart` events of a program come from seeks and are dropped. A program that fails to load reports nothing; giving up on it is issue #34.

## The watchdog

`poll_event` checks MPV each time it is called. At most once every 3 seconds (`LIVENESS_INTERVAL`) it also sends `get_property pid` and waits up to `command_timeout` for the answer, so a hung MPV is found even when the tuner sends nothing. When the process has exited, the socket closed, or a command or that check got no answer within `command_timeout`, the player kills what is left of MPV and starts a new one, at most once a second (`RESTART_DELAY`). When the new MPV is connected, `poll_event` returns `Restarted`. Commands sent while MPV is down return a `PlayerError`.

`poll_event` does this work itself, so it can block:

- The liveness check blocks for up to `command_timeout` (5 s) when MPV does not answer.
- A restart blocks for up to `start_timeout` (10 s) while the new MPV opens its socket. While MPV keeps failing to open it, the attempt repeats about once a second.

`Tuner::pump` calls `poll_event`, so it stalls for as long as these take, and so does whatever loop calls `pump`.

The player keeps no copy of earlier commands to replay. On `Restarted` the tuner tunes the current channel again (static, a tune-in clip, then the program at its live position), because the schedule and the channel are the tuner's. Overlays are lost too; whoever showed them shows them again.

## Tests

`src/player/mpv/tests.rs` tests the protocol against a fake MPV, a Unix socket in a test thread, including the liveness check and a fake that stops answering. The `real` tests run the `mpv` binary headless on clips made by `ffmpeg`: a clip then a queued program at its offset, the key reaching a local HTTP server as a header and not in the URL or MPV's arguments, a killed MPV coming back, a stopped (`SIGSTOP`) MPV being replaced without any command, the stand-by card still playing after 4 seconds and then replaced by a clip, and the card switching MPV's `glsl-shaders` to the CRT shader and back, with its torn and settled overlays accepted. They print a note and pass when `mpv` or `ffmpeg` is missing, as on CI. The fake-MPV tests check the card's exact commands and ASS, the escaping of its text, that each other command removes the overlay first, the fall back to static when MPV refuses the overlay, the shader switches onto and off the card, the global shaders after a restart (a script stands in for MPV), and the glitch tearing and restoring the band only while the card is up.

Not tested here: the Pi with hardware decoding and the TV, including killing MPV during playback there, the CPU and GPU cost of static, the stand-by card and the CRT shader on the Pi, and how the card and its filters look on the TV.
