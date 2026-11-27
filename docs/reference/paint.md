# Drawing the interface

The code is in `src/paint/`. It draws the TV's interface in the Cable Grid design ([cable-grid.md](../design/cable-grid.md)) into bitmaps the player shows as overlays, and runs the CRT pass on them. The SCTV watermark, the channel banner, the program guide and the Picture menu draw with it.

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

## The banner

`paint::banner::draw(&banner)` returns the channel banner as an overlay across the bottom of a 1920x1080 screen, before the CRT pass. The band is 176 px tall, 96 px in from each side and 86 px up from the bottom (`banner::LEFT`, `BOTTOM`, `HEIGHT`), with a 16 px transparent margin (`banner::MARGIN`) for the glow.

| Part | Contents |
| --- | --- |
| Channel cell, 276 px | The number with two digits at 108 px and the name at 34 px, yellow with the hard shadow, centred |
| Main panel | The violet field; an ochre strip 56 px tall with a slanted right end, carrying `times` at 32 px in ochre ink; the title at 64 px in white |
| Clock cell | `clock` in VCR OSD Mono at 46 px, centred |

Text that does not fit is cut with an ellipsis (`Font::fit`).

## The guide

`paint::guide::draw(&view)` draws a `GuideView` as a full-screen overlay, before the CRT pass. The run loop builds the view from the schedule (`run::guide`).

| Part | Where | Contents |
| --- | --- | --- |
| Info panel | the top-left quarter, on the violet field | four centred lines at 52 px: the channel, the focused program in quotes (yellow), the day, `Channel N` |
| Video | the top-right quarter | left transparent for the picture |
| Header | y 556, 52 px tall | the clock's cell, then three half-hour slots in yellow |
| Date row | y 614, 40 px tall | the date across the slots |
| Rows | from y 660, four rows 86 px tall, drawn separately by `draw_rows` | a channel cell (two-digit number and name in yellow) and program cells: series blue, films maroon, the focused one in the focus colour with a 6 px yellow outline. A program that runs past an edge gets a white arrow there |

The grid runs from 96 px to 1824 px across, inside the area a TV may cut off. Titles that do not fit are cut with an ellipsis, and a sliver of a cell gets no title. `paint::guide::draw_rows(rows)` draws every row in one bitmap at the grid's place, 92 px apart (`guide::PITCH`, a multiple of 4), so the guide can show four of them (`guide::VISIBLE`, 368 px) and scroll through the rest. `paint::guide::draw_clock(text)` draws the clock cell, in VCR OSD Mono, as its own small overlay at the head of the grid.

## The menu

`paint::menu::draw(&view)` draws a `MenuView` as a full-screen, opaque overlay, before the CRT pass: the violet field; the mark in yellow beside an ochre heading band with a slanted end (the title at 48 px); a bevelled list panel with a 76 px row per setting, its name on the left and its value on the right at 44 px, the focused row in the focus colour and yellow; a line under the list describing the focused setting; and the keys in VCR OSD Mono under a rule at the bottom.

## The mark

`paint::mark::shapes()` is the Guide Wave with SC as its middle row, as one path on the 64-unit grid of `assets/brand/mark.svg`. The shapes run from (`mark::LEFT`, `mark::TOP`) = (9, 11) and are `mark::WIDTH` by `mark::HEIGHT` = 47 by 42 units.

## The watermark

`paint::watermark::draw(form)` returns the SCTV bug at full opacity as an overlay at its place on a 1920x1080 screen:

| Form | Shape | Size |
| --- | --- | --- |
| `Form::Beside` | the mark, then SCTV with a capital height of 24 of the mark's 42 units, tracked 0.04 em | 72 px tall |
| `Form::Under` | the mark, then SCTV 11 units tall and tracked 0.14 em under it, centred | about 100 px tall |

Both are white with a black shadow at 50% alpha, 3 px right and down. The bug's right edge is 96 px from the screen's right edge and its top 86 px from the top. The bitmap has a transparent margin of 16 px (`watermark::MARGIN`) round it for the CRT pass's glow.

`paint::fade(&mut overlay, opacity)` scales every byte of an overlay, colour and alpha together; the run loop uses it to show the bug at `watermark::OPACITY` (60%) and to fade it. When and which form shows is in [run.md](run.md#the-watermark).

## Tests

`src/paint/tests.rs` checks the fonts, text placement and shadows, bevels, gradients, the overlay's byte order, and the pass: Off, transparency, the scanline rows and their phase, Heavy against Light, glow reach, the colour fringe and premultiplied output; the mark's bounds, the watermark's size, position and clear margin, fading, the banner's place and a long title cut with an ellipsis. With `SURFCULT_PAINT_PREVIEW=<dir>`, `cargo test --lib paint::tests::preview` also writes a sample drawing as `paint-flat.png`, `paint-heavy.png` and `paint-light.png` to look at, and `paint::tests::preview_watermark` writes both watermark forms over a sample picture with the pass at Heavy and Off, `paint::tests::preview_banner` writes the banner, `paint::tests::preview_guide` writes the guide over a picture in the video's quarter, and `paint::tests::preview_menu` writes the menu.
