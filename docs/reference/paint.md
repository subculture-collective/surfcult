# Drawing the interface

The code is in `src/paint/`. It draws the TV's interface in the Cable Grid design ([cable-grid.md](../design/cable-grid.md)) into bitmaps the player shows as overlays, and runs the CRT pass on them. Nothing draws with it yet; the watermark, banner and guide will.

## Canvas

`Canvas::new(width, height)` is a transparent bitmap. Coordinates are pixels from the top left, and anything outside is clipped.

| Call | What it draws |
| --- | --- |
| `fill_rect` | A flat rectangle. |
| `fill_gradient` | A rectangle with a vertical gradient, for the violet field and the ochre band. |
| `bevel` | A bevelled cell: a fill, a light edge along the top and left, a dark one along the bottom and right. |
| `outline` | An outline inside a rectangle, for the yellow focus box. |
| `fill_path` | A `tiny_skia::Path` with a transform, for the mark. |
| `text` | One line of text with an optional hard `Shadow`, returning its width. |

`to_overlay(x, y)` returns the canvas as a `player::Overlay` at that screen position: premultiplied blue, green, red and alpha, as MPV's `overlay-add` takes it.

Colours are `Rgba` with straight alpha; the design's tokens are the constants in `paint::palette`.

## Fonts

`Font::archivo()` is Archivo Narrow Bold, for titles, labels and headings. `Font::vcr()` is VCR OSD Mono, for clocks and menu footers. Both are compiled in from `assets/fonts/` (sources and licences in its README). `width` measures a line, `cap_height` gives the height of a capital, and `outline` turns a line into a path. Characters a font lacks are skipped, and kerning is not applied.

## The CRT pass

`paint::crt::apply(&mut overlay, level)` runs the pass on an overlay before it is shown. `Crt` is `Heavy` (the default), `Light` or `Off`; `Crt::parse` reads `heavy`, `light` or `off` in any case. Off leaves the overlay as drawn.

| Step | Heavy | Light |
| --- | --- | --- |
| Horizontal blur, weight of each neighbour | 51/256 | 31/256 |
| Red left, blue right | 2 px | 1 px |
| Glow: a quarter-size copy, box-blurred (radius 2) twice each way, scaled back up and screened over | 55% | 30% |
| Third and fourth row in every four screen rows | 80%, 40% | 85%, 55% |

- Scanlines follow the screen's rows: the overlay's `y` sets their phase, so an overlay anywhere on the screen lines up with the others. A bitmap that scrolls must move in steps of 4 rows.
- Scanlines darken colour only. Alpha stays, so the video never shows through in stripes.
- Glow can raise the alpha of transparent pixels near the interface, so it spills a few pixels onto the video. Leave a margin of about 16 pixels around a small overlay such as the watermark for it.
- Every step keeps colour at or below alpha. An overlay whose buffer does not match its size is left alone.

On the development Mac (Apple M3, release build) a full 1920x1080 pass takes about 29 ms, and a 260x140 watermark 0.5 ms. It has not been measured on the Pi 4B, which is several times slower; the pass runs only when the interface is redrawn, never per video frame.

## Tests

`src/paint/tests.rs` checks the fonts, text placement and shadows, bevels, gradients, the overlay's byte order, and the pass: Off, transparency, the scanline rows and their phase, Heavy against Light, glow reach, the colour fringe and premultiplied output. With `SURFCULT_PAINT_PREVIEW=<dir>`, `cargo test --lib paint::tests::preview` also writes a sample drawing as `paint-flat.png`, `paint-heavy.png` and `paint-light.png` to look at.
