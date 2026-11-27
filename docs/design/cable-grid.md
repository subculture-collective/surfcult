# Cable Grid: the TV and web design

This is the design for surfcult's TV screens and web admin. The owner chose it on November 27, 2026 after four review rounds, recorded in the Studio study `studio/studies/2026-11-27-surfcult-screen-directions/` in `subcult-tv`.

It is a specification. The drawing layer, the CRT pass, the SCTV watermark, the banner, the guide with its auto-scroll and the Picture menu with the CRT setting are built ([paint.md](../reference/paint.md), [run.md](../reference/run.md#the-guide)). The web admin is not.

## Decisions

| Date | Decision |
| --- | --- |
| 2026-11-27 | Direction A, Cable Grid, in its violet palette. |
| 2026-11-27 | A CRT pass on the TV interface layer only; never on the video or the web admin. Heavy is the default; viewers can choose Light or Off. |
| 2026-11-27 | The guide scrolls on its own after the remote has been idle. A remote key takes over, and scrolling resumes after the next idle period. |
| 2026-11-27 | Mark: Guide Wave with SC as the middle row. Watermarks: SCTV beside the mark and SCTV under the mark. |
| 2026-11-27 | The watermark is semi-transparent, appears on landing on a channel, fades after a while, and stays on during commercials. |
| 2026-11-27 | The owner states that the VCR OSD Mono licence is granted. |
| 2026-11-27 | The guide starts scrolling after 5 seconds without a remote key. The watermark holds for 3 seconds after landing on a channel, then fades over 1 second. |

Choices marked *proposed* below were made by the agent and are open to change.

## Palette

| Token | Value | Role |
| --- | --- | --- |
| `field-top` | `#2e2180` | Top of the violet field gradient, the mark's tile |
| `field-bottom` | `#1a1150` | Bottom of the field gradient |
| `cell` | `#3a2ca6` | Channel cells, headers, menu rows, web panels |
| `focus` | `#4a3cc4` | The focused cell or row, under the yellow outline |
| `bevel-light` | `#8e84ff` | Top and left bevel edge |
| `bevel-dark` | `#0e0833` | Bottom and right bevel edge, inset fields |
| `series` | `#2646c4` | Program cells for series |
| `movie` | `#7c2144` | Program cells for films |
| `music` | `#1c6c86` | Program cells for music and mixed blocks |
| `yellow` | `#ffd23f` | Channel numbers, times, the focus outline, the mark |
| `ochre-top`, `ochre-bottom` | `#f7c64c`, `#de7f1c` | Heading bands and the web header bar |
| `ochre-ink` | `#2a1200` | Text on ochre |
| `white` | `#ffffff` | Titles and body text on the field |
| `shadow` | `#05020f` | The hard text shadow |

Contrast before the CRT pass: white text is at least 5.9:1 on every cell colour, and yellow at least 5.3:1 on `cell`, `focus`, `series` and `movie`. Yellow on `music` is 4.1:1, so titles on music cells stay white.

## Type

- **Archivo Narrow Bold** sets everything on the TV and the web's headings, buttons and labels. Mixed case for titles; capitals for channel IDs, headings and key hints.
- **Archivo** (400 and 600) sets web body text and form fields.
- **VCR OSD Mono** sets clocks and the footer line of TV menus.

Both TV fonts are in `assets/fonts/`, with their sources and licences.

TV text is at least 28 px tall at 1920x1080 so it reads from the couch. TV text has a hard shadow 3 px right and 3 px down in `shadow`.

## Mark and assets

`tools/brand/build_marks.py` builds these from fixed geometry and Archivo Narrow Bold outlines; the files need no font. Rebuild with the command in the script's docstring.

| File | Use |
| --- | --- |
| `assets/brand/mark.svg` | The full-colour mark on its violet tile: app icons, the web header, TV menu headings |
| `assets/brand/mark-mono.svg` | One colour (`currentColor`), no tile: print, embossing, anywhere colour is fixed |
| `assets/brand/favicon.svg` | The 16-pixel drawing on the pixel grid; use it at 16 and 32 pixels only |
| `assets/brand/lockup-on-dark.svg` | Mark and SURFCULT on the violet field |
| `assets/brand/lockup-on-light.svg` | Mark and SURFCULT in `#1a1150` on light grounds |
| `assets/brand/watermark-beside.svg` | The SCTV bug, beside form, white |
| `assets/brand/watermark-under.svg` | The SCTV bug, under form, white |

The mark is the guide's three rows: the top row curls over like a breaking wave, the middle row is the letters S and C built from the same bars, and the bottom row is ochre. Do not redraw the letters in a font, recolour the rows separately, or set the mark on a field without its tile except in one colour. Below 24 pixels use `favicon.svg`, whose letters are drawn pixel by pixel.

## TV screens

All TV interface is drawn by Rust into a 1920x1080 premultiplied BGRA bitmap and shown with `show_overlay`. MPV draws overlays after its shaders, so the video's own filters never touch the interface ([player.md](../reference/player.md#the-crt-shader)).

- **Guide.** The top half: an info panel on the left (channel name, the title in quotes, day and channel number, centred) and the program in a window on the right. The bottom half: the grid. The clock cell (`VCR OSD Mono`) and three half-hour time cells head it, a date row sits under them, then channel rows of number over ID and program cells. Cells are bevelled; a program that continues past the grid edge gets a white arrow. The remote moves a yellow outline across the cells. As built, the guide gives the video the top-right quarter, so the info panel and the video each take half the width, and the remote moves the focus between channels only; moving through time needs left and right keys, which the key map does not have yet.
- **Auto-scroll.** After 5 seconds without a remote key, the rows scroll upward on their own and loop. Any remote key stops the scroll and puts the focus back where it was; the idle timer starts again. As built, the key that stops the scroll does nothing else. Scroll steps are whole multiples of 4 rows; see the CRT pass.
- **Banner.** A band across the bottom: a channel cell (number large, name under it), a main panel with an ochre strip carrying the times and the title under it, and a clock cell. It keeps the earlier banner's timing and content rules. Not built yet: a `NEXT` line under the title.
- **Menus.** A full-screen violet field, an ochre heading band with the mark, a list panel of rows with the focused row in `focus` and yellow, and a footer line with the date, time and key hints (`▲▼ CHOOSE  ◀▶ CHANGE  BACK CLOSE`).

## The CRT pass

The pass runs on the interface bitmap after it is drawn and before it goes to MPV, only when the interface redraws. It never touches the video. It is separate from the `crt` GLSL shader, which runs on the whole picture and is too slow for video on the Pi.

1. **Soften and misconverge.** A horizontal blur of 0.9 px, then the red channel shifts 2 px left and the blue 2 px right.
2. **Glow.** A 480x270 copy, box-blurred twice, scaled back up and added at 55%. Glow may spill a few pixels past a panel onto the video.
3. **Scanlines.** In every 4 rows, row 3 is darkened to 40% and row 2 to 80%. Darken colour, not alpha, or the video shows through in stripes.
4. **Registration.** The guide scrolls by moving MPV's offset into a taller bitmap, so every scroll step must be a multiple of 4 rows to keep the lines on the screen's grid.

Those are the Heavy values. Light uses a 0.6 px blur, a 1 px shift, glow at 30%, and rows darkened to 55% and 85%. Off skips the pass.

Left out: curvature and vignetting (the video would have to bend too), noise, flicker and rolling bars (they need a redraw every video frame), and a phosphor mask (moiré once the TV scales 1080p to 4K). The cost on the Pi has not been measured.

## The SCTV watermark

The watermark is the station bug, in the top-right corner of every channel. It is its own small overlay, not part of a full-screen bitmap, so showing and fading it rewrites only a small bitmap.

- **Look.** White at 60% opacity, with a black shadow at 50% offset 3 px right and down. The CRT pass applies to it at the chosen level.
- **Place and size.** Its right edge is 96 px from the screen's right edge and its top 86 px from the top. The beside form is 72 px tall (the mark 72 px, SCTV beside it); the under form is 100 px tall.
- **On landing on a channel.** When a channel change settles, the moment the banner shows, the beside form appears and holds for 3 seconds, then fades out over 1 second.
- **During commercials.** For the whole of a break event the under form shows instead, and fades out over 1 second when the break ends. Landing on a channel during a break shows the under form until the break ends.
- **Filler and idents** (*proposed*). Filler is treated like a break. Idents show no watermark, because an ident is itself the station's identification.
- **Guide and menus.** The watermark hides while the guide or a menu is open and comes back if a break is still on when it closes.
- **Fading.** A fade is about 8 rewrites of the small bitmap over the second, each with the alpha scaled down.

## Settings

The TV's Picture menu (the menu key) offers the CRT filter: Heavy (the default), Light or Off. The remote has no left or right keys, so OK steps to the next level. The choice is stored and outlives a restart; until the viewer chooses, `SURFCULT_CRT` sets the level. The guide's 5-second auto-scroll delay is fixed. The web admin, when it exists, should offer the same setting.

## Web admin

The web admin uses the same tokens, type, bevels and grid, with no CRT pass. An ochre header bar carries the mark, SURFCULT and the navigation; the current page is a bevelled `cell` tab with yellow text. The guide is the same grid with times inside each cell, and editing happens in a bevelled side panel.

- Fields are inset (dark bevel top-left, light bottom-right) on `bevel-dark`; focus is a 3 px yellow outline.
- Primary buttons are ochre with `ochre-ink` text; secondary buttons are `cell` with white text. Pressed buttons invert the bevel.
- Errors sit under the field in `#ffc2ad` and name the conflict and the fix, for example "Laff Track already uses 05. Pick a free number."
- Hover brightens a cell or button; keyboard focus is always visible.
- At phone width the admin becomes the remote: an "On now" list with a progress bar per channel and a Tune button.
