# Stand-by card generator

`gen.py` makes the stand-by card's picture and lettering, which the player compiles in:

- `assets/standby/bars.lavfi`: the colour bars, a `lavfi` URL (`color` plus one `drawbox` per colour region, 640x360, 30 frames a second). It is `player::mpv::STANDBY_SOURCE`.
- `assets/standby/text.drawing`: PLEASE STANDBY as one ASS vector drawing (`\p3`, quarter pixels of a 1920x1080 screen), the cyan copy of the text. `src/player/mpv/card.rs` draws the red copy and the white overlap from it.

Both were matched to the owner's reference picture (612x408). `fit.json` holds the result of that fit:

- `P`: for each letter, its left edge in reference pixels and its horizontal scale;
- `T`: for each letter, its weight between Oswald Regular (0) and Bold (1), about 0.54 to 0.67 (SemiBold);
- `base` and `sy`: the baseline and the vertical scale shared by all letters;
- `shift`: how far the reference's red channel is shifted right, in reference pixels (7.33).

The measured colour regions, the band rectangle and a few hand corrections to single letters (each moves a few points so the glyph edges land within 1 pixel of the reference at 1080p) are in `gen.py` itself. The fitting scripts are not kept; they need the reference picture, which is not in the repository.

## Regenerating

Get the static Oswald Regular and Bold TTF files, version 4.103 (the ones used had SHA-256 `bd6ff5a8…500e46` and `a6ee8b91…7bd68`). They must be point-compatible, as instances of the variable font are. Then, from the repository root:

```bash
python3 tools/standby/gen.py Oswald-Regular.ttf Oswald-Bold.ttf
```

It needs only the Python standard library (`ttf.py` reads the TrueType outlines). It prints the band rectangle, the start of the red strip and the red copy's shift in `\p3` units; if those change, update `BAND`, `FRINGE_LEFT` and `RED_SHIFT` in `src/player/mpv/card.rs` (today 594 1025 7086 1898, 502 and 92). Run `make verify` afterwards: the card's unit tests compare against the drawing.

## Licence

The lettering is derived from Oswald, Copyright 2016 The Oswald Project Authors (https://github.com/googlefonts/OswaldFont), licensed under the SIL Open Font License 1.1. The licence is in `OFL.txt` here and must stay with the derived outlines.
