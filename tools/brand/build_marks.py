"""Build surfcult's mark, favicon, lockups and SCTV watermarks as SVG.

Writes, relative to the repository root, into assets/brand/:

- mark.svg: the Guide Wave with SC as the middle row, full colour, 64x64;
- mark-mono.svg: the same shapes in one colour (currentColor), no tile;
- favicon.svg: the 16-pixel version, drawn on the pixel grid;
- lockup-on-dark.svg and lockup-on-light.svg: the mark and SURFCULT;
- watermark-beside.svg and watermark-under.svg: the white SCTV station bugs.

Text is converted to outlines, so the files need no font. Usage, from the
repository root:

    uv run --with fonttools python tools/brand/build_marks.py 'ArchivoNarrow[wght].ttf'

The font is the variable Archivo Narrow from Google Fonts; the build uses its
700 (Bold) instance. Kerning is not applied; the strings are capitals.
The lettering is derived from Archivo Narrow, Copyright 2019 The Archivo
Narrow Project Authors, under the SIL Open Font License 1.1 (OFL.txt here).
"""

import os
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(os.path.dirname(HERE)), 'assets', 'brand')

VIOLET = '#2e2180'      # tile and field
YELLOW = '#ffd23f'      # top row and letters
OCHRE = '#de7f1c'       # bottom row
HIGHLIGHT = '#fff3b8'   # 2-unit bevel on the top row
OCHRE_LIGHT = '#f7c64c' # 2-unit bevel on the bottom row
INK = '#1a1150'         # one-colour mark on light grounds
WHITE = '#ffffff'

# The mark's shapes on a 64-unit grid: top row curling over, S and C built
# from 3-unit bars, bottom row.
TOP = 'M9 11H43A13 13 0 0 1 56 24V33H46V24A3 3 0 0 0 43 21H9Z'
S = 'M9 26h11v3h-8v2h8v8h-11v-3h8v-2h-8z'
C = 'M23 26h11v3h-8v7h8v3h-11z'
BOTTOM = 'M9 44h47v9h-47z'

# The 16-pixel version, on whole pixels.
TOP16 = 'M2 2h9v1h1v1h1v1h1v4h-2v-3h-1v-1h-9z'
S16 = 'M2 6h3v1h-2v1h2v3h-3v-1h2v-1h-2z'
C16 = 'M6 6h3v1h-2v3h2v1h-3z'
BOTTOM16 = 'M2 12h12v2h-12z'


def svg(w, h, body, title):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:g}" height="{h:g}" '
            f'viewBox="0 0 {w:g} {h:g}" role="img" aria-label="{title}">\n'
            f'<title>{title}</title>\n{body}\n</svg>\n')


def mark_full(x=0, y=0):
    t = f'transform="translate({x:g} {y:g})" ' if x or y else ''
    return (f'<g {t}>'
            f'<rect width="64" height="64" rx="12" fill="{VIOLET}"/>'
            f'<path d="{TOP}" fill="{YELLOW}"/><rect x="9" y="11" width="34" height="2" fill="{HIGHLIGHT}"/>'
            f'<path d="{S}" fill="{YELLOW}"/><path d="{C}" fill="{YELLOW}"/>'
            f'<path d="{BOTTOM}" fill="{OCHRE}"/><rect x="9" y="44" width="47" height="2" fill="{OCHRE_LIGHT}"/>'
            '</g>')


def mark_mono(fill, x=0, y=0, scale=1):
    t = f'transform="translate({x:g} {y:g}) scale({scale:g})" '
    return f'<path {t}fill="{fill}" d="{TOP} {S} {C} {BOTTOM}"/>'


class Type:
    """Archivo Narrow Bold outlines as SVG path data."""

    def __init__(self, path):
        font = TTFont(path)
        if 'fvar' in font:
            font = instantiateVariableFont(font, {'wght': 700})
        self.font = font
        self.upm = font['head'].unitsPerEm
        self.cap = font['OS/2'].sCapHeight
        self.glyphs = font.getGlyphSet()
        self.cmap = font.getBestCmap()

    def size_for_cap(self, cap_height):
        return cap_height * self.upm / self.cap

    def width(self, text, size, tracking_em=0.0):
        s = size / self.upm
        adv = sum(self.glyphs[self.cmap[ord(ch)]].width for ch in text) * s
        return adv + tracking_em * size * (len(text) - 1)

    def path(self, text, size, x, baseline, tracking_em=0.0):
        s = size / self.upm
        pen = SVGPathPen(self.glyphs, ntos=lambda v: f'{v:.2f}'.rstrip('0').rstrip('.'))
        for ch in text:
            g = self.glyphs[self.cmap[ord(ch)]]
            g.draw(TransformPen(pen, (s, 0, 0, -s, x, baseline)))
            x += g.width * s + tracking_em * size
        return pen.getCommands()


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    t = Type(sys.argv[1])
    os.makedirs(OUT, exist_ok=True)
    files = {}

    files['mark.svg'] = svg(64, 64, mark_full(), 'surfcult')
    files['mark-mono.svg'] = svg(64, 64, mark_mono('currentColor'), 'surfcult')
    files['favicon.svg'] = svg(16, 16, (
        '<g shape-rendering="crispEdges">'
        f'<rect width="16" height="16" rx="3" fill="{VIOLET}"/>'
        f'<path d="{TOP16} {S16} {C16}" fill="{YELLOW}"/><path d="{BOTTOM16}" fill="{OCHRE}"/></g>'),
        'surfcult')

    # Lockup: SURFCULT with a cap height of 30 units, centred on the mark.
    size = t.size_for_cap(30)
    word_x = 64 + 14
    word_w = t.width('SURFCULT', size, 0.02)
    w = round(word_x + word_w, 2)
    d = t.path('SURFCULT', size, word_x, 47, 0.02)
    files['lockup-on-dark.svg'] = svg(w, 64, mark_full() + f'<path fill="{WHITE}" d="{d}"/>', 'surfcult')
    files['lockup-on-light.svg'] = svg(w, 64, mark_mono(INK) + f'<path fill="{INK}" d="{d}"/>', 'surfcult')

    # The watermarks use the mark's shapes without the tile's margin: 47 x 42
    # units, from (9, 11) to (56, 53). One unit of room below for overshoot.
    mw, mh = 47, 42

    # Beside: SCTV with a cap height of 24, centred on the mark.
    size = t.size_for_cap(24)
    tw = t.width('SCTV', size, 0.04)
    d = t.path('SCTV', size, mw + 9, (mh + 24) / 2, 0.04)
    files['watermark-beside.svg'] = svg(round(mw + 9 + tw, 2), mh + 1, mark_mono(WHITE, -9, -11) + f'<path fill="{WHITE}" d="{d}"/>', 'SCTV')

    # Under: SCTV with a cap height of 11, widely spaced, 6 units below.
    size = t.size_for_cap(11)
    tw = t.width('SCTV', size, 0.14)
    w = round(max(mw, tw), 2)
    d = t.path('SCTV', size, (w - tw) / 2, mh + 6 + 11, 0.14)
    files['watermark-under.svg'] = svg(w, mh + 6 + 11 + 1, mark_mono(WHITE, (w - mw) / 2 - 9, -11) + f'<path fill="{WHITE}" d="{d}"/>', 'SCTV')

    for name, text in files.items():
        with open(os.path.join(OUT, name), 'w') as f:
            f.write(text)
        print(f'assets/brand/{name}')


if __name__ == '__main__':
    main()
