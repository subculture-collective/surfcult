"""Generate the stand-by card's picture and lettering from fit.json.

Writes, relative to the repository root:

- assets/standby/bars.lavfi: the lavfi URL of the colour bars (640x360, 30 fps);
- assets/standby/text.drawing: PLEASE STANDBY as one ASS vector drawing
  (\\p3: 1/4-pixel units on a 1920x1080 screen), the cyan copy of the text.

Usage: python3 tools/standby/gen.py OSWALD_REGULAR.ttf OSWALD_BOLD.ttf

The reference picture is 612x408. Its R channel is shifted right by SHIFT
pixels relative to G and B, so every colour edge has a fringe, and the text
is two copies (cyan = G/B, red = R) that add up to white where they overlap.
The player draws the red copy and the white overlap from the cyan drawing
(src/player/mpv/card.rs); the band and fringe rectangles printed at the end
are constants there.
"""

import json
import os
import sys

from geom import Oswald

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
FIT = json.load(open(os.path.join(HERE, 'fit.json')))
SHIFT = FIT['shift']          # R-channel shift, in reference pixels
VW, VH = 640, 360             # generated video size
RW, RH = 612, 408             # reference size
SX, SY = 1920 / RW, 1080 / RH
TEXT = 'PLEASESTANDBY'        # the letters, in order; the space is in fit.json's x positions
BAND = (40.0, 564.67, 96.79, 179.22)  # G/B band x0, x1, y0, y1 in reference pixels

# Colour regions measured from the reference: rows of (x where a colour
# starts, (r, g, b)), clean G/B edges before the R shift.
ROWS = [
    (0, 275.39, [(0, (255, 255, 255)), (83.83, (255, 254, 3)), (171.35, (45, 212, 254)),
                 (258.48, (2, 255, 4)), (344.95, (254, 3, 207)), (433.5, (254, 0, 2)),
                 (520.9, (3, 2, 254))]),
    (275.39, 306.38, [(0, (3, 2, 254)), (83.82, (1, 1, 1)), (171.39, (254, 3, 207)),
                      (258.47, (1, 1, 1)), (345.1, (172, 254, 242)), (433.59, (1, 1, 1)),
                      (520.9, (181, 181, 181))]),
    (306.38, 408, [(0, (3, 2, 129)), (93.71, (255, 255, 255)), (203.13, (3, 2, 129)),
                   (312.84, (1, 1, 1))]),
]


def colour_at(segs, x):
    c = segs[0][1]
    for edge, col in segs:
        if x >= edge:
            c = col
    return c


def bars():
    """The lavfi graph: a background colour and one drawbox per region,
    each region's colour being R from x - SHIFT and G/B from x."""
    boxes = []
    for y0, y1, segs in ROWS:
        edges = sorted(set([e for e, _ in segs] + [e + SHIFT for e, _ in segs if e > 0] + [RW]))
        top, bottom = round(y0 * VH / RH), round(y1 * VH / RH)
        for a, b in zip(edges, edges[1:]):
            mid = (a + b) / 2
            gb, r = colour_at(segs, mid), colour_at(segs, mid - SHIFT)
            x0, x1 = round(a * VW / RW), round(b * VW / RW)
            if x1 > x0:
                boxes.append((x0, top, x1 - x0, bottom - top, (r[0], gb[1], gb[2])))
    merged = []
    for b in boxes:
        p = merged[-1] if merged else None
        if p and p[1] == b[1] and p[4] == b[4] and p[0] + p[2] == b[0]:
            merged[-1] = (p[0], p[1], p[2] + b[2], p[3], p[4])
        else:
            merged.append(b)

    def hexc(c):
        return '0x%02X%02X%02X' % c
    chain = [f'color=c={hexc(merged[0][4])}:s={VW}x{VH}:r=30', 'format=rgb24']
    chain += [f'drawbox=x={x}:y={y}:w={w}:h={h}:c={hexc(c)}:t=fill' for x, y, w, h, c in merged[1:]]
    return 'av://lavfi:' + ','.join(chain)


def units(v):
    """Pixels on a 1920x1080 screen to \\p3 units (1/4 pixel)."""
    return int(round(v * 4))


def text_subpaths(font):
    """The cyan copy of the text as subpaths of [command, [(x, y), ...]] in
    \\p3 units. Quadratic curves become ASS cubic 'b' segments."""
    subs = []
    for i, ch in enumerate(TEXT):
        x0, sx = FIT['P'][i]
        for contour in font.transform(ch, FIT['T'][i], x0, sx, FIT['base'], FIT['sy']):
            sub = []
            for s in contour:
                pts = [(q[0] * SX, q[1] * SY) for q in s[1:]]
                if not sub:
                    sub.append(['m', [(units(pts[0][0]), units(pts[0][1]))]])
                if s[0] == 'L':
                    sub.append(['l', [(units(pts[1][0]), units(pts[1][1]))]])
                else:
                    p0, c, p1 = pts
                    k1 = (p0[0] + 2 / 3 * (c[0] - p0[0]), p0[1] + 2 / 3 * (c[1] - p0[1]))
                    k2 = (p1[0] + 2 / 3 * (c[0] - p1[0]), p1[1] + 2 / 3 * (c[1] - p1[1]))
                    sub.append(['b', [(units(x), units(y)) for x, y in (k1, k2, p1)]])
            subs.append(sub)
    return subs


def parse(drawing):
    out = []
    tokens = drawing.split()
    i = 0
    while i < len(tokens):
        c = tokens[i]
        i += 1
        n = []
        while i < len(tokens) and tokens[i] not in ('m', 'l', 'b'):
            n.append(int(tokens[i]))
            i += 1
        out.append([c, list(zip(n[0::2], n[1::2]))])
    return out


def correct(subs):
    """Hand corrections from the second fitting round: each one moves a few
    points so the glyph edges land within 1 px of the reference at 1080p."""
    def find(x, y):
        for k, s in enumerate(subs):
            if s[0][1][0] == (x, y):
                return k
        raise KeyError((x, y))

    def mapx(k, f):
        for seg in subs[k]:
            seg[1] = [(f(x, y), y) for x, y in seg[1]]

    def mapy(k, f):
        for seg in subs[k]:
            seg[1] = [(x, f(x, y)) for x, y in seg[1]]

    # L: foot length.
    mapx(find(1661, 1761), lambda x, y: 1956 if x == 1908 else x)
    # N: redrawn with a new diagonal.
    subs[find(4777, 1761)] = parse('m 4777 1761 l 4777 1163 l 4916 1163 l 5024 1523 l 5024 1163 l 5120 1163 '
                                   'l 5120 1761 l 5010 1761 l 4872 1316 l 4872 1761 l 4777 1761')
    # E: stem widths.
    mapx(find(1987, 1761), lambda x, y: 2095 if x == 2119 else x)
    mapx(find(3068, 1761), lambda x, y: 3176 if x == 3202 else x)
    # P: lower bowl and counter.
    mapy(find(1294, 1761), lambda x, y: round(y + (y - 1334) / 165 * 19) if 1334 < y <= 1499 else y)
    mapy(find(1407, 1418), lambda x, y: round(y + (y - 1333) / 85 * 12) if y > 1333 else y)
    # A: wider counters, higher apex.
    subs[find(2452, 1554)] = parse('m 2438 1554 l 2533 1554 l 2488 1288 l 2438 1554')
    subs[find(4514, 1554)] = parse('m 4500 1554 l 4595 1554 l 4550 1288 l 4500 1554')
    # T: crossbar.
    mapx(find(4127, 1761), lambda x, y: {4032: 4016, 4338: 4360}.get(x, x))
    # Y: crotch.
    k = find(6060, 1761)
    for seg in subs[k]:
        seg[1] = [(6118, 1376) if p == (6118, 1398) else p for p in seg[1]]
    # D: counter bottom.
    mapy(find(5297, 1683), lambda x, y: round(y - (y - 1573) / 110 * 12) if y > 1573 else y)
    # S of STANDBY: top-right terminal.
    mapx(find(3850, 1769), lambda x, y: x + 12 if x > 3960 and 1230 < y < 1330 else x)
    return subs


def emit(subs):
    return ' '.join(c + ' ' + ' '.join(f'{x} {y}' for x, y in pts) for s in subs for c, pts in s)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    font = Oswald(sys.argv[1], sys.argv[2])
    out = os.path.join(ROOT, 'assets', 'standby')
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, 'bars.lavfi'), 'w') as f:
        f.write(bars() + '\n')
    with open(os.path.join(out, 'text.drawing'), 'w') as f:
        f.write(emit(correct(text_subpaths(font))) + '\n')
    bx0, bx1, by0, by1 = BAND
    print('band (\\p3 units):', units((bx0 + SHIFT) * SX), units(by0 * SY), units(bx1 * SX), units(by1 * SY))
    print('red fringe from x:', units(bx0 * SX), ' red copy shift:', units(SHIFT * SX))


if __name__ == '__main__':
    main()
