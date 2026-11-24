"""Oswald glyph outlines, interpolated between Regular and Bold, in pixels."""

from ttf import TTF


class Oswald:
    """Two point-compatible static Oswald files: Regular (400) and Bold (700)."""

    def __init__(self, regular, bold):
        self.a = TTF(regular)
        self.b = TTF(bold)

    def glyph(self, ch, t):
        """The contours of `ch` at weight t (0 = Regular, 1 = Bold)."""
        ca = self.a.contours(self.a.cmap[ch])
        cb = self.b.contours(self.b.cmap[ch])
        return [[(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2]) for a, b in zip(c1, c2)]
                for c1, c2 in zip(ca, cb)]

    def transform(self, ch, t, x0, sx, base, sy):
        """Segment lists of `ch` in pixels: the glyph's left edge at x0, its
        baseline at `base`, scaled by sx and sy."""
        cs = self.glyph(ch, t)
        gx0 = min(p[0] for c in cs for p in c)

        def f(p):
            return (x0 + (p[0] - gx0) * sx, base - p[1] * sy)
        return [[(s[0],) + tuple(f(q) for q in s[1:]) for s in segments(c)] for c in cs]


def segments(contour):
    """('L', p0, p1) and ('Q', p0, control, p1) of a closed TrueType contour."""
    pts = contour
    k = next((i for i, p in enumerate(pts) if p[2]), None)
    if k is None:
        p0 = ((pts[0][0] + pts[1][0]) / 2, (pts[0][1] + pts[1][1]) / 2, 1)
        pts = [p0] + pts[1:] + [pts[0]]
        k = 0
    pts = pts[k:] + pts[:k]
    n = len(pts)
    out = []
    cur = pts[0][:2]
    ctrl = None
    for i in range(1, n + 1):
        p = pts[i % n]
        if p[2]:
            if ctrl is None:
                out.append(('L', cur, p[:2]))
            else:
                out.append(('Q', cur, ctrl, p[:2]))
                ctrl = None
            cur = p[:2]
        else:
            if ctrl is None:
                ctrl = p[:2]
            else:
                mid = ((ctrl[0] + p[0]) / 2, (ctrl[1] + p[1]) / 2)
                out.append(('Q', cur, ctrl, mid))
                cur = mid
                ctrl = p[:2]
    if ctrl is not None:
        out.append(('Q', cur, ctrl, pts[0][:2]))
    return out
