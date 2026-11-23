# Pi spike scripts

Phase 0 measurement scripts for the Raspberry Pi 4B. See `docs/research/2026-11-22-pi-playback-spike.md` for what they found and `docs/runbooks/raspberry-pi.md` for how to reach the Pi.

Copy the folder to `~/spike/` on the Pi. The Jellyfin scripts expect `~/spike/.jf-key` and `~/spike/jf.conf`, both mode 600, and `~/spike/jf_items.json` from `jf_pick.py`. Nothing here contains a key, and none of it prints one.

| Script | Purpose |
| --- | --- |
| `playtest.sh` | Play a local file to the TV and print the decoder, drops and CPU. |
| `tune.py` | Time-to-picture for direct play and transcodes against Jellyfin. |
| `overlay.py` | CPU and drops with a full-screen scrolling bitmap overlay. |
| `gapless.py` | Gap between the end of one item and the start of the next. |
| `mpvipc.py` | Small MPV JSON IPC client used by `overlay.py` and `gapless.py`. |
| `jf_stats.py` | Count the library's codecs, resolutions, frame rates and audio. Runs anywhere on the LAN. |
| `jf_pick.py` | Pick one representative item per category into `jf_items.json`. |
