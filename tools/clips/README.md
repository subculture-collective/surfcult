# SCTV clip generator

`make_clips.py` makes SCTV's own interstitial clips in the Cable Grid design ([docs/design/cable-grid.md](../../docs/design/cable-grid.md)): the palette, Archivo Narrow Bold with the hard shadow, VCR OSD Mono, and the mark and lockup from `assets/brand/`. It writes one directory per clip pool:

| Directory | Clips | Use |
| --- | --- | --- |
| `stingers/` | 6, 4 s each | The guide's three rows sweep in and the mark lands, over a rising synth hit. |
| `bumps/` | 16, 11 s each | Lines in the station's voice fade in on the violet field over a synth pad, with the SCTV band at the bottom. |
| `idents/` | 2, 20 s each | "We'll be right back" and "Up next" slates. As a `breaks.pad` pool they fill a break's shortfall, and they can stop anywhere. |
| `sign-off/` | 1, 58 s | The nightly sign-off: the lockup, four closing cards over an original slow chorale, then colour bars with a 1 kHz tone, fading to black. |

The music and tones are synthesised in the script, so the clips carry no third-party material. Every clip is 1920x1080 at 30 frames a second, H.264 High in yuv420p with AAC stereo at 48 kHz, normalised to -16 LUFS: what the Pi decodes in hardware.

The bump lines are in `BUMPS` and the pad chords in `CHORDS`; add a line or change one and run the script again. The sign-off's cards say programming resumes at 6:00 a.m.; change `cards` in `make_sign_off` if a channel's `until` differs.

## Running it

It needs `uv`, `ffmpeg` and `rsvg-convert` on `PATH`; `uv` installs Pillow and NumPy for the run. From the repository root:

```bash
uv run tools/clips/make_clips.py clips-out
ONLY=bump uv run tools/clips/make_clips.py clips-out   # one kind: sign-off, sting, ident or bump
```

The whole set takes a few minutes and is about 12 MB. The output is generated media, so keep it out of the repository: copy each directory to a clip pool on the Pi (for example `/home/patrick/surfcult/clips/bumps`) and name the pools in `channels.toml`.
