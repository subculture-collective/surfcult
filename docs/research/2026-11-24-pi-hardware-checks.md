# Pi hardware checks, 2026-11-24

What the checks in `tests/pi.rs` found on the Raspberry Pi 4B (Raspberry Pi OS Debian 13, MPV 0.40.0, FFmpeg 7.1.5, 1920x1080 at 60 Hz over HDMI, `throttled=0x0`, 49 to 56 C) with Jellyfin 12.4.0 on Almaz. They cover issues #21 (playback URLs), #22 (MPV watchdog) and #72 (stand-by card and display filters). The look on the TV is not covered: nobody watched it.

## #22: MPV is restarted when it dies or hangs

The player started MPV with the Pi flags, played a 1080p H.265 clip (`hwdec-current` `drm`) and then had its MPV killed.

| Fault | Noticed | New MPV | Old process | Plays again |
| --- | --- | --- | --- | --- |
| `kill -9` | 0.4 s | new pid | gone | yes, 2.5 s in after 3 s |
| `SIGSTOP` (hang) | 5.6 s (the liveness check plus its 5 s timeout) | new pid | gone | yes |

Passed.

## #21: Jellyfin playback URLs play

The check asked Jellyfin for each URL kind, played a 4 s tune-in clip, queued the program 10 minutes in, and measured 10 s of playback. The key never appeared in MPV's command line.

| Item | URL | Picture after queue (4 s of it is the clip) | Position at start | Decoder | MPV CPU | Dropped |
| --- | --- | --- | --- | --- | --- | --- |
| HEVC 8-bit 1080p, 24 fps | direct | 6.6 s | 600.0 s | `drm` | 0.25 cores | 0 |
| HEVC 10-bit 1080p, 24 fps | direct | 5.1 s | 600.1 s | `drm` | 0.25 cores | 0 |
| AV1 10-bit 1080p | transcode | 5.8 s | 600.1 s | `v4l2m2m` | 0.27 cores | 0 |
| H.264 10-bit 1080p | transcode | 6.8 s | 600.1 s | `v4l2m2m` | 0.28 cores | 0 |

A transcode starts at the requested position: time 0 in MPV is 10 minutes into the media, and the player reports 600 s. `stop_transcode` was accepted (204), and on Almaz the `jellyfin-ffmpeg` process was gone seconds later, so Jellyfin honours the `DeviceId=surfcult` that surfcult sets.

The check found three problems that the fake Jellyfin had hidden. They are fixed with it:

1. **No user, no playback info.** Jellyfin 12.4.0 answers `400 Error processing request.` to a `PlaybackInfo` request that carries a device profile but no `UserId`. An API key has no user. The client now sends a `UserId`: `SURFCULT_JELLYFIN_USER_ID`, or else the first enabled user from `GET /Users`.
2. **An empty `SubtitleProfiles` makes every item with a subtitle transcode.** An HEVC item that should direct-play was transcoded, with all three reasons (video, audio and subtitle codec) reported. Jellyfin then burned the subtitle in, and that transcode took 70 s to start. Listing every subtitle format as `External` fixed it; `Drop` and `Embed` did not. With it HEVC and H.264 items direct-play and AV1, H.264 10-bit and 4K HDR items transcode, as ADR 0001 expects. The player now sets `sid=no` for programs so no subtitle is shown.
3. Without the burn-in, transcodes start in under 2 s after the clip.

## #72: the stand-by card and the CRT shader

Each row is 20 s of the stand-by card (30 frames a second) on a 60 Hz output, before `--fbo-format=rgba8` was added.

| Case | MPV CPU | Frames dropped (of 600) |
| --- | --- | --- |
| Card, no shader | 0.50 cores | 0 |
| Card, CRT shader | 0.47 | 243 |
| Card, CRT and glitch (the default) | 0.46 | 205 |
| Card, CRT also as a global filter (no double application) | 0.44 | 176 |

The player's own loop, polling every 20 ms for the glitch, used about 0.006 cores.

What the experiments that followed showed:

- **MPV's default 16-bit intermediate format is the cost.** An empty user shader dropped 132 of 450 card frames in 15 s; with `--fbo-format=rgba8` it dropped none. With `rgba8` the CRT shader drops 45 of 600 in 20 s (7.5%) on the `mpv` command line, and lighter variants (no rolling band, one texture read) 15 to 21. `rgba8` is now in `PI_OUTPUT_ARGS`. The player itself, rerun with it, dropped 123 (CRT), 56 (CRT and glitch, the default) and 52 (CRT also global) of 600, so expect 9 to 20%.
- **A global `crt` is too heavy for video.** On a 24 frames a second 1080p H.265 file: no shader 0 of 480 dropped; CRT 176 (37%); CRT with `rgba8` 65 (13.5%); the lighter variant 64. On 60 frames a second files 550 to 620 of 1200. Hardware decoding stayed on in every case. The V3D GPU cannot do a full-screen pass at 1080p for video. Keep `crt` on the card only, which is the default.
- **`vo=gpu-next` is not an option.** It ran the card's shader with no drops, but with `--hwdec=drm` MPV 0.40 fails to import frames ("Failed to duplicate dmabuf fd: Too many open files") after a few seconds.
- Without any shader, 60 frames a second H.264 files already dropped 190 to 280 frames in 15 s (`v4l2m2m`) and 10-bit H.265 about 50. The owner's library is 92% at 24 frames a second or lower.
- One rerun of the card measurements (with the old binary, by mistake) showed two global-CRT video rows with no drops at low CPU, which does not fit the other runs. The probable cause is a watchdog restart of MPV in the middle of the measurement; those rows are not trusted, and the rerun with the final code gave 400 to 800 drops for the same rows.

Screenshots of what MPV drew are saved by the check (`/tmp/surfcult-pi/` on the Pi).

## Still to do

- Watch the card on the TV: the curvature, scanlines, colour and glitch, and whether the remaining 9 to 20% of dropped card frames (30 frames a second on a 60 Hz output) show.
- The check ran with `--ao=null`; sound over HDMI was verified separately (see `raspberry-pi.md`).
