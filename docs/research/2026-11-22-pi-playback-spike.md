# Pi 4B playback spike (Phase 0)

Date: 2026-11-22. Device: Raspberry Pi 4B, 2 GB, Debian 13, MPV 0.40, FFmpeg 7.1, output to a TV at 1920x1080 at 60 Hz. Source: the owner's Jellyfin 12.4.0 on Almaz over the LAN. This is the Phase 0 spike in the [design](../superpowers/specs/2026-11-22-surfcult-design.md).

## Result

Phase 0 passes on this hardware, with two conditions and one limit.

1. MPV must run with `--profile=fast`. Its defaults overload the Pi's GPU.
2. Use `--hwdec=drm` for H.265 and `--hwdec=v4l2m2m` for H.264.
3. Transcodes must come from Jellyfin as a progressive MPEG-TS stream with a start offset. HLS misses the 15-second limit.

The design's three thresholds:

| Threshold | Result |
| --- | --- |
| A direct-play tune takes 3 s or less | Pass. H.265 plays in 0.3 to 0.7 s. H.264 in MP4 takes 0.6 to 1.0 s from the start but 2.9 s at an offset, so it only just passes. |
| A transcoded tune is hidden by a tune-in clip of 15 s or less | Pass for 1080p over progressive TS: 4 to 5 s for AV1, 6 to 7 s for H.265. Fail over HLS (19 s) and for 4K HDR (13 to 25 s progressive). |
| A full-screen guide overlay uses at most about one core and scrolls smoothly | Pass. A 1080p bitmap scrolled at 10 updates per second costs 0.5 cores and at 30 per second about 1.1 cores, with no dropped video frames. The text-only fallback is not needed. |

24 fps content, which is 92% of the library, plays cleanly. 60 fps content is not reliable, even in hardware.

## Method

- Clips: the public Jellyfin test videos (1080p, 60 fps, H.264 and H.265 8- and 10-bit, 5 and 20 Mbps), checked against their published SHA-256 sums. To get 24 fps content, the 20 Mbps clips were re-encoded to 24 fps at about 8 Mbps.
- Each run played to the TV through `--vo=gpu --gpu-context=drm`, waited 5 seconds, then sampled for 20 seconds. Drops and the decoder in use come from MPV's JSON IPC socket. CPU is the process's CPU time divided by wall time, so 1.0 is one full core of four.
- Throttling was checked: the clock held 1.8 GHz, temperature peaked at 70 C, and no throttle flags were set.

## Playback at 24 fps (output drops in 20 s, about 480 frames)

| Codec | Decode | Default profile | `--profile=fast` |
| --- | --- | --- | --- |
| H.264 | `v4l2m2m` zero-copy | 257 drops | 0 drops, 0.15 cores |
| H.264 | `v4l2m2m-copy` | 170 drops | 0 drops, 0.56 cores |
| H.265 8-bit | software | 82 drops | 0 drops, 1.54 cores |
| H.265 8-bit | `drm-copy` | not run | 0 drops, 0.58 cores |
| H.265 10-bit | software | 200 drops | 0 drops, 3.29 cores |
| H.265 10-bit | `drm-copy` | not run | 0 drops, 0.65 cores |
| H.265 10-bit | `drm` zero-copy | not run | 0 drops, 0.20 cores |

Software H.265 10-bit plays but uses most of the CPU, which leaves nothing for the guide overlay or a 30 fps file. Hardware decode removes that problem.

## Playback at 60 fps (`--profile=fast`, about 1200 frames)

| Codec | Decode | Drops | CPU |
| --- | --- | --- | --- |
| H.264 20 Mbps | `v4l2m2m-copy` | 662 | 0.83 cores |
| H.264 20 Mbps | `v4l2m2m` zero-copy | 300 | 0.46 cores |
| H.265 10-bit 20 Mbps | `drm-copy` | 307 | 1.08 cores |
| H.265 10-bit 20 Mbps | `drm` zero-copy | 69 | 0.44 cores |
| H.265 8-bit 5 Mbps | software | 87 | 3.55 cores |
| H.265 10-bit 5 Mbps | software | 587 | 3.68 cores |

Zero-copy is better in every pairing. The Pi's hardware H.264 decoder tops out near 60 fps at 1080p, so a 60 fps H.264 file has no headroom.

## What went wrong on the way

- A first test used `--hwdec=v4l2m2m` for H.265. That wrapper cannot drive the Pi's H.265 decoder, so MPV fell back to software and the first conclusion was "H.265 is software only". That was wrong. The installed `libavcodec` contains `hevc_v4l2request`, and MPV reaches it through `--hwdec=drm`.
- A first software benchmark used a synthetic test pattern and reported 2.9 times real time. Real clips decoded at 0.87 to 1.5 times real time at 60 fps. Use real content for every later test.
- Frame drops looked like a decoder problem and were not. They came from MPV's default scaler and shader settings.

## The Jellyfin library

Counted through the API on 2026-11-22: 1,440 movies, 170 series and 4,656 episodes. Jellyfin returned stream data for 5,090 video items and none for the other 4,811, which were not investigated, so the shares below describe those 5,090.

| Property | Share |
| --- | --- |
| Video codec | H.265 79% (43% 10-bit, 36% 8-bit), H.264 14%, AV1 6% (302 items), other 1% |
| Resolution | 1080p 94%, 720p 2%, SD 3%, 4K 16 items |
| Frame rate | 24 fps or lower 92%, 29.97 to 30 fps 8%, 48 fps or higher 17 items |
| Dynamic range | SDR 99%, HDR or Dolby Vision 53 items |
| Default audio | AAC 42%, E-AC3 25%, AC3 19%, Opus 10%, TrueHD 2%, DTS 2%, FLAC 2% |

The Pi plays H.265 and H.264 8-bit directly, so about 90% of the library direct-plays. The rest needs a transcode: AV1, H.264 10-bit (42 items), VVC (4), 4K and HDR, and TrueHD audio. The design does not say how small "small" is. About 10% is, in my judgment, small enough that its device profile stands. The profile should exclude AV1.

## Tune time against Jellyfin

Each row is the time from loading a stream into a running MPV until it reports a configured output and active playback, polled every 30 ms. Several numbers mean several runs. The streams are real films from the library. Transcodes are to H.264 at 8 Mbps and 1080p.

| Case | Seconds | Decode | CPU (cores) |
| --- | --- | --- | --- |
| Direct H.265 10-bit, from the start | 0.26, 0.51, 0.26 | `drm` | 0.31 |
| Direct H.265 10-bit, 40% in | 0.45, 0.38, 0.56 | `drm` | 0.28 |
| Direct H.265 8-bit, 40% in | 0.72, 0.48 | `drm` | 0.29 |
| Direct H.264 in MP4, from the start | 0.6, 0.97 | `v4l2m2m` | 0.31 |
| Direct H.264 in MP4, 40% in | 2.9, 2.87 | `v4l2m2m` | 0.11 |
| Direct H.265 10-bit with TrueHD 6 channel, 40% in | 0.66, 0.62 | `drm` | 0.40 |
| Direct 4K HDR10 H.265, 40% in | 0.94, 0.9 | `drm` | 0.28, but 53 of about 144 frames dropped in 6 s |
| Direct AV1 10-bit, 40% in | 5.98, 5.95 | software | 3.01 |
| Transcode AV1, HLS, from the start | 1.69, 2.51, 3.52 | `v4l2m2m` | 0.16 |
| Transcode AV1, HLS, 40% in | 19.2, 20.0, 19.0 | `v4l2m2m` | 0.14 |
| Transcode 4K HDR, HLS, 40% in | 54.0, 30.6 | `v4l2m2m` | 0.17 |
| Transcode AV1, progressive TS, from the start | 1.64, 1.5, 2.02 | `v4l2m2m` | 0.15 |
| Transcode AV1, progressive TS, 40% in | 4.63, 4.36, 4.05 | `v4l2m2m` | 0.16 |
| Transcode H.265 1080p, progressive TS, 40% in | 7.11, 6.69, 6.2 | `v4l2m2m` | 0.47 |
| Transcode 4K HDR, progressive TS, 40% in | 25.2, 16.9, 12.7 | `v4l2m2m` | 0.30 |

Notes:

- HLS is slow at an offset because the playlist covers the whole film and MPV's seek into it makes Jellyfin restart the transcode at a far segment. Jellyfin serves that segment in about 4.7 s when asked directly, so most of the 19 s is spent in the player. A progressive stream that carries `StartTimeTicks` avoids it.
- The H.264 MP4 at an offset is slower than the same file from the start, probably because the MP4 index has to be fetched. That was not confirmed.
- The transcodes ran on Almaz while it did other work, and which encoder it used was not checked. Treat them as typical, not best case.
- Direct-play AV1 works in software at 3 cores. That leaves nothing for the guide, so AV1 goes through a transcode.

## Seeking and the second item

- Starting at an offset works through MPV's `start` option. The numbers above include it.
- A second item appended to MPV's playlist starts 0.5 to 1.05 s after the first ends. Playlist prefetch trims 0 to 0.3 s of that. The slowest pair was H.265 into H.264, which switches decoders. A short ident or bumper hides gaps this size.

## The guide overlay

A random full-screen 1920x1080 BGRA bitmap with transparency, drawn through MPV's `overlay-add` over 24 fps H.265 10-bit playback, scrolled by changing the offset into a taller bitmap held in `/dev/shm`. Each scenario ran 12 seconds.

| Scenario | MPV cores | Updates per second | Dropped video frames |
| --- | --- | --- | --- |
| No overlay | 0.28 | 0 | 0 |
| Static overlay | 0.22 | 0 | 0 |
| Scroll at 10 per second | 0.48 | 10 | 0 |
| Scroll at 30 per second | 1.11 | 30 | 0 |
| Scroll at 60 per second (requested) | 1.47 | 36 | 0 |

An IPC call takes 20 to 27 ms, so 60 updates per second cannot be reached through it. A guide that scrolls a row at a time needs far fewer.

## Audio, CEC and input

- Audio over HDMI: MPV opens the ALSA device `plughw:CARD=vc4hdmi0,DEV=0` as stereo 16-bit and plays without errors. Whether sound is audible was not checked.
- CEC: the adapter (`vc4-hdmi-0`) works and the Pi registers as Playback Device 1 at physical address 1.0.0.0. The TV does not acknowledge any CEC message, so HDMI-CEC is probably disabled in the TV's settings. Until it is enabled, CEC remote keys cannot be tested.
- Input without a desktop: the CEC adapter appears as a keyboard input device (`/dev/input/event2`), and `patrick` can read it through the `input` group. No key presses were captured.

## Jellyfin details the client needs

- Jellyfin 12.4.0 accepts an API key only in the header `Authorization: MediaBrowser Token="KEY"`. `X-Emby-Token` and the `api_key` query parameter return 401.
- A direct-play URL is `/Videos/{id}/stream?static=true&mediaSourceId={source}`.
- A transcode URL is `/Videos/{id}/stream.ts` with `VideoCodec=h264`, `AudioCodec=aac`, `VideoBitrate`, `maxWidth`, `maxHeight`, `StartTimeTicks` and a fresh `PlaySessionId`.
- The key lives on the Pi at `~/spike/.jf-key` with mode 600 and is passed to MPV through a config file, not on the command line, so it does not appear in the process list.

## Still open

- Whether Jellyfin keeps an item's id when Tdarr replaces its file. It would need a file to be replaced.
- Whether sound is audible, and CEC and remote keys, once CEC is enabled on the TV.
- Long-duration playback, restart recovery and network loss.
- The 4K HDR tune of 13 to 25 s, if those 16 items matter.

## Harness

The scripts are in `tools/pi-spike/` and are meant to run on the Pi from `~/spike/`. `playtest.sh` plays a local file and prints the decoder, drops and CPU. `tune.py` measures tune time against Jellyfin, `overlay.py` the guide overlay and `gapless.py` the gap between items. `jf_stats.py` counts the library and `jf_pick.py` chooses test items. They read the Jellyfin key from a file and never print it.
