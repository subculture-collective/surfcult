# Player

The `Player` trait in `src/player/` is the screen as the tuner sees it: show static, play a local clip, queue a program, seek, stop, and show or clear an overlay. `FakePlayer` records these for tests. `MpvPlayer` in `src/player/mpv/` sends them to a real MPV process over its JSON IPC socket.

## Starting MPV

`MpvPlayer::start(MpvConfig, resolver)` starts MPV and connects to its socket. `MpvConfig::new(runtime_dir)` gives the Pi setup.

| Field | Default | Meaning |
| --- | --- | --- |
| `binary` | `mpv` | The executable, found on `PATH`. |
| `runtime_dir` | (required) | A directory only this user can open, mode 700. It holds `mpv.sock` and the overlay bitmaps. It is created if missing. An existing directory that others can open is refused. |
| `output_args` | `PI_OUTPUT_ARGS` | `--profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --hwdec=drm,v4l2m2m --force-window=yes` (ADR 0001, decision 2). Tests use `--vo=null --ao=null`. |
| `static_source` | `STATIC_SOURCE` | What static plays, in a loop: grey noise from FFmpeg's `lavfi` at 320x180 and 25 fps, scaled up by MPV. |
| `command_timeout` | 5 s | How long a command waits for MPV's answer. |
| `start_timeout` | 10 s | How long a new MPV may take to open its socket. |

Every MPV also gets `--no-config --idle=yes --input-terminal=no --ytdl=no --keep-open=no --prefetch-playlist=yes --msg-level=all=error --input-ipc-server=<runtime_dir>/mpv.sock`. MPV's errors go to surfcult's stderr. A Unix socket path is limited to 107 bytes, so keep `runtime_dir` short (for example `$XDG_RUNTIME_DIR/surfcult`).

Hardware decoding: `--hwdec=drm,v4l2m2m` asks for the Pi's H.265 decoder, then its H.264 decoder. MPV decodes in software when neither works. `auto-safe` is not used because only this pair was measured on the Pi.

## Commands

| Trait method | MPV command |
| --- | --- |
| `show_static` | `loadfile <static_source> replace` with `loop-file=inf` |
| `play_clip(path)` | `loadfile <absolute path> replace` |
| `queue_program(media, start_ms)` | `set_property http-header-fields [...]`, then `loadfile <url> append` with `start=<seconds>` |
| `seek(position_ms)` | `seek <seconds> absolute` |
| `stop` | `stop` |
| `show_overlay(id, overlay)` | writes the BGRA pixels to `overlay-<id>.bgra` in the runtime directory, then `overlay-add <id> <x> <y> <file> 0 bgra <w> <h> <w*4>` |
| `clear_overlay(id)` | `overlay-remove <id>` |

`replace` empties MPV's playlist, so static and a clip drop anything queued. A program is appended behind the clip and opened early by the playlist prefetch. It is not preloaded paused, because one MPV process cannot hold a second file paused (ADR 0001, decision 5).

Overlay layers are numbered 0 to 63. The pixels are blue, green, red and alpha bytes, with the colour already multiplied by alpha, and there must be exactly `width * height * 4` of them. Overlays stay up when the item on screen changes.

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

`end-file` for static or a program, with any other reason (`stop`, `redirect`, `error`), or for an entry a later command replaced, reports nothing. Later `playback-restart` events of a program come from seeks and are dropped. A program that fails to load reports nothing; giving up on it is issue #34.

## The watchdog

`poll_event` checks MPV each time it is called. At most once every 3 seconds (`LIVENESS_INTERVAL`) it also sends `get_property pid` and waits up to `command_timeout` for the answer, so a hung MPV is found even when the tuner sends nothing. When the process has exited, the socket closed, or a command or that check got no answer within `command_timeout`, the player kills what is left of MPV and starts a new one, at most once a second (`RESTART_DELAY`). When the new MPV is connected, `poll_event` returns `Restarted`. Commands sent while MPV is down return a `PlayerError`.

`poll_event` does this work itself, so it can block:

- The liveness check blocks for up to `command_timeout` (5 s) when MPV does not answer.
- A restart blocks for up to `start_timeout` (10 s) while the new MPV opens its socket. While MPV keeps failing to open it, the attempt repeats about once a second.

`Tuner::pump` calls `poll_event`, so it stalls for as long as these take, and so does whatever loop calls `pump`.

The player keeps no copy of earlier commands to replay. On `Restarted` the tuner tunes the current channel again (static, a tune-in clip, then the program at its live position), because the schedule and the channel are the tuner's. Overlays are lost too; whoever showed them shows them again.

## Tests

`src/player/mpv/tests.rs` tests the protocol against a fake MPV, a Unix socket in a test thread, including the liveness check and a fake that stops answering. The `real` tests run the `mpv` binary headless on clips made by `ffmpeg`: a clip then a queued program at its offset, the key reaching a local HTTP server as a header and not in the URL or MPV's arguments, a killed MPV coming back, and a stopped (`SIGSTOP`) MPV being replaced without any command. They print a note and pass when `mpv` or `ffmpeg` is missing, as on CI.

Not tested here: the Pi with hardware decoding and the TV, including killing MPV during playback there, and the CPU cost of the static source on the Pi.
