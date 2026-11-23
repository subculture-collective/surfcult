# Pi 4B playback spike, first measurements

Date: 2026-11-22. Device: Raspberry Pi 4B, 2 GB, Debian 13, MPV 0.40, FFmpeg 7.1, output to a TV at 1920x1080 at 60 Hz. This covers the decode and output part of Phase 0 in the [design](../superpowers/specs/2026-11-22-surfcult-design.md). It does not cover tune time, a Jellyfin source, seeking, a preloaded second item or the guide overlay. Those are still open.

## Result

The first risk in the design is closed: the Pi 4B decodes H.264 and H.265 on the GPU through MPV, including 10-bit H.265, and plays 24 fps content cleanly. Two conditions apply.

1. MPV must run with `--profile=fast`. Its defaults overload the Pi's GPU.
2. Use `--hwdec=drm` for H.265 and `--hwdec=v4l2m2m` for H.264.

60 fps content is not reliable, even in hardware. The library is expected to be mostly film and TV, but Phase 0 should confirm the frame-rate mix when it counts the Jellyfin library.

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

## Still to measure

- Tune time against Jellyfin direct play (limit 3 seconds) and a transcode (hidden by a clip of up to 15 seconds).
- Seek to an offset and a preloaded second item.
- One overlay frame and the guide: CPU cost and smoothness. This is the design's largest bet.
- The library's codec, resolution, bit depth, HDR and frame-rate counts.
- Audio over HDMI, and remote and CEC input without a desktop session.

## Harness

The test script is `~/spike/playtest.sh` on the Pi. It takes a file, an MPV hwdec name and a duration, plays to the TV and prints the decoder in use, drops and CPU. Set `EXTRA="--profile=fast"` for the fast profile and `MODE=1920x1080@60` to force the display mode.
