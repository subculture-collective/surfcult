# ADR 0001: What the Pi 4B and the library showed in Phase 0

## Status

Accepted, 2026-11-23. It follows from the measurements in issues #10 and #11. The design document is updated to match, and it links here where it differs from the original assumptions.

## Context

The design assumed a Raspberry Pi 4B could decode H.265 and H.264 in hardware, direct-play most of the owner's Jellyfin library, hide a transcode behind a tune-in clip, and draw a full-screen guide overlay without a heavy CPU cost. It named five risks and set three thresholds. Phase 0 tested them.

Setup: Raspberry Pi 4B, 2 GB, Debian 13, MPV 0.40, FFmpeg 7.1, TV at 1920x1080 at 60 Hz. Jellyfin 12.4.0 on Almaz over the LAN. The Pi numbers and method are in [the playback spike](../research/2026-11-22-pi-playback-spike.md).

### The library (issue #10)

A read-only query of Jellyfin's own database covered 6,137 playable movies and episodes (5,067 hours). With the design's device profile, 91.0% direct play. Of the rest, 7.3% (445 items) need a video transcode and 1.8% (108 items) only an audio transcode.

- Video transcodes: AV1 330 items (5.4% of the library, and most of the transcodes), HDR or Dolby Vision 56, above 1080p 43, VC-1 7, VVC 4, MPEG-2 3, VP9 1, MPEG-1 1.
- Audio only: 105 items have TrueHD as the default track and 3 have PCM.
- Mix: HEVC 4,561, H.264 1,230, AV1 330. 5,738 are 1080p. 2,955 are 10-bit, almost all HEVC.
- A separate pass through the API found 92% of items at 24 fps or lower and 17 items at 48 fps or higher (it saw stream data for 5,090 items, so treat that figure as approximate).

### The Pi (issue #11)

| Threshold | Result |
| --- | --- |
| Direct-play tune of 3 s or less | Pass for H.265 (0.3 to 0.7 s) and H.264 8-bit (0.6 to 1.0 s, 2.9 s at an offset). Fails for H.264 10-bit (3.7 s, software decode). |
| Transcoded tune hidden by 15 s or less | Pass for 1080p over a progressive MPEG-TS stream (4 to 7 s). Fails over HLS (about 19 s) and for 4K HDR (13 to 25 s). |
| Full-screen guide overlay at about one core or less | Pass. A 1080p bitmap scrolled at 30 updates per second costs about 1.1 cores and at 10 per second 0.5 cores, with no dropped video frames. |

Other facts that matter:

- Hardware decode works for H.265 including 10-bit (`--hwdec=drm`, 0.2 cores at 24 fps) and for H.264 8-bit (`--hwdec=v4l2m2m`). The Pi 4B cannot hardware-decode H.264 10-bit or AV1.
- MPV's default video settings overload the GPU and drop about a third of the frames, even with hardware decode. `--profile=fast` fixes it.
- 24 fps content plays cleanly. 60 fps content does not, even in hardware.
- One MPV process cannot hold a second file paused. `--prefetch-playlist` opens the next file early, and the next item then starts 0.5 to 1.05 s after the first ends.
- Jellyfin 12.4.0 accepts the API key only as `Authorization: MediaBrowser Token="KEY"`. `X-Emby-Token` and `api_key` return 401.
- MPV decodes TrueHD in software. One 5.1 TrueHD film direct-played in 0.66 s using about 0.1 more cores than a film with E-AC3.

## Decision

1. **Keep the design.** One MPV process plays the watched channel, Jellyfin is the source, the scheduler is independent, the guide is a bitmap overlay, and v1 outputs 1080p. The text-only guide fallback is not needed.
2. **Run MPV with** `--profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --hwdec=drm,v4l2m2m`. Hardware decoding is requested and MPV falls back to software on its own. The Jellyfin key goes to MPV through a config file (`--include`), not on the command line.
3. **Change the device profile.** Direct play: H.265 8-bit and 10-bit and H.264 8-bit, up to 1080p SDR; audio AAC, MP3, FLAC, Opus, AC3, E-AC3, DTS and TrueHD (MPV downmixes to stereo by default). Transcode: AV1, H.264 10-bit, VVC, VC-1, MPEG-2, VP9, MPEG-1, anything above 1080p, and HDR or Dolby Vision. This moves about 42 H.264 10-bit items to transcoding and TrueHD back to direct play. That leaves about 8% of the library needing a video transcode and about 92% direct-playing. The TrueHD change rests on one item, so confirm it when HDMI sound is checked.
4. **Transcode as a progressive stream.** Ask Jellyfin for `/Videos/{id}/stream.ts` with H.264 and AAC, a 1080p cap, about 8 Mbps and `StartTimeTicks`. Do not use HLS at an offset.
5. **Queue the program behind the tune-in clip; do not preload it paused.** Append the program to MPV's playlist at its offset and let prefetch open it while the clip plays. Choose the clip length from the measured start-up cost: about a second for direct play, 4 to 7 s for a transcode. Allow for a slow first transcode (one cold start took 18.7 s), so the sequence must wait for the program and not promise a time.
6. **Draw the guide with `overlay-add`.** Plan for at most about 30 updates per second; an IPC call takes 20 to 27 ms, so about 36 per second is the ceiling. A guide that scrolls a row at a time needs far fewer.
7. **Accept three limits for v1.** 60 fps content may drop frames (the library is mostly 24 fps). A 4K HDR transcode may take 13 to 25 s, and about 16 items are 4K. A transcoded start after a long pause may be slow.

## Consequences

- The tuner and player issues (#19, #22, #23) build on a queue-behind-the-clip sequence and need tests for a late program and a slow transcode. The design's "loaded paused" wording is replaced.
- The Jellyfin issues (#18, #21) send the header form above, never put the key in a URL or process arguments, and build a transcode URL with `StartTimeTicks`.
- The device profile in code follows decision 3, not the original design text.
- Sound over HDMI, CEC and remote keys, whether Jellyfin keeps an item's id when Tdarr replaces its file, and long-duration and network-loss behaviour are not covered by this record. HDMI-CEC is off on the owner's TV, so remote keys cannot be tested until it is on.
