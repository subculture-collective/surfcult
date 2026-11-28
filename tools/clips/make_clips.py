# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow>=11", "numpy>=2"]
# ///
"""Make surfcult's own interstitial clips in the Cable Grid style.

Writes into OUT_DIR (default ./clips-out), one directory per pool kind:

- stingers/: 4-second SCTV stings: the guide's rows sweep in and the mark
  lands, over a rising synth hit;
- bumps/: 11-second text bumps in the station's voice, line by line on the
  violet field, over a soft synth pad;
- idents/: 20-second "we'll be right back" slates that pad a short break,
  quiet enough to stop anywhere;
- sign-off/: the nightly sign-off: the station card, an original slow brass
  chorale under the closing cards, then colour bars and a 1 kHz tone.

Every clip is 1920x1080 at 30 fps, H.264 yuv420p with AAC stereo, loudness
normalised to -16 LUFS, which is what the Pi plays in hardware. Usage, from
the repository root (needs ffmpeg and rsvg-convert on PATH):

    uv run tools/clips/make_clips.py [OUT_DIR]

The output is generated media: keep it out of the repository and copy the
directories to the Pi's clip pools.
"""

import math
import os
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
FONT = str(ROOT / "assets/fonts/ArchivoNarrow-Bold.ttf")
MONO = str(ROOT / "assets/fonts/VCR_OSD_MONO_1.001.ttf")
W, H, FPS, RATE = 1920, 1080, 30, 48_000

# The Cable Grid palette (docs/design/cable-grid.md).
FIELD_TOP = (0x2E, 0x21, 0x80)
FIELD_BOTTOM = (0x1A, 0x11, 0x50)
CELL = (0x3A, 0x2C, 0xA6)
BEVEL_LIGHT = (0x8E, 0x84, 0xFF)
BEVEL_DARK = (0x0E, 0x08, 0x33)
SERIES = (0x26, 0x46, 0xC4)
MOVIE = (0x7C, 0x21, 0x44)
MUSIC = (0x1C, 0x6C, 0x86)
YELLOW = (0xFF, 0xD2, 0x3F)
OCHRE_TOP = (0xF7, 0xC6, 0x4C)
OCHRE_BOTTOM = (0xDE, 0x7F, 0x1C)
OCHRE_INK = (0x2A, 0x12, 0x00)
WHITE = (0xFF, 0xFF, 0xFF)
SHADOW = (0x05, 0x02, 0x0F)

BUMPS = [
    ["You're watching SCTV.", "It's late.", "We're glad you're still here."],
    ["This is a bump.", "It exists so you can get a snack.", "Go. We'll wait."],
    ["Somewhere a VCR is blinking 12:00.", "It isn't our fault."],
    ["The guide scrolls by itself now.", "You don't have to do anything.", "You never did."],
    ["Thirty channels.", "Nothing on.", "Perfect."],
    ["We don't know what you're watching.", "We like it that way."],
    ["Remember when TV", "just happened to you?", "It still can."],
    ["No algorithm.", "Just a schedule", "and whatever's next."],
    ["If you're seeing this at 3 a.m.:", "hi."],
    ["Stay tuned.", "Or don't.", "The channel will be here."],
    ["This broadcast is coming to you", "from a small computer", "behind the TV."],
    ["Adjust your antenna.", "Kidding.", "There's no antenna."],
    ["Commercial breaks are a feature.", "Ask anyone from 1994."],
    ["You could change the channel.", "But what if the next thing is good?"],
    ["The following program", "has not been rated", "by anyone."],
    ["Surf the culture.", "SCTV."],
]

# Pad chords for the bumps, as frequencies in Hz: soft, open voicings.
CHORDS = [
    [130.81, 196.00, 246.94, 329.63],  # Cmaj7
    [110.00, 164.81, 261.63, 329.63],  # Am(add9)-ish
    [146.83, 220.00, 277.18, 369.99],  # D(add#11)
    [123.47, 185.00, 233.08, 311.13],  # Bm-ish
    [98.00, 146.83, 246.94, 293.66],   # G(add9)
    [116.54, 174.61, 233.08, 349.23],  # Bb
]


def font(size, mono=False):
    return ImageFont.truetype(MONO if mono else FONT, size)


def field():
    """The violet field: a vertical gradient from field-top to field-bottom."""
    t = np.linspace(0.0, 1.0, H)[:, None, None]
    top = np.array(FIELD_TOP, dtype=float)
    bottom = np.array(FIELD_BOTTOM, dtype=float)
    rows = (top + (bottom - top) * t).astype(np.uint8)
    return Image.fromarray(np.repeat(rows, W, axis=1), "RGB")


def raster(svg, width):
    """`svg` rendered at `width` pixels wide, as RGBA."""
    with tempfile.NamedTemporaryFile(suffix=".png") as png:
        subprocess.run(["rsvg-convert", "-w", str(width), "-o", png.name, svg], check=True)
        return Image.open(png.name).convert("RGBA").copy()


def text(draw, xy, s, size, fill=WHITE, anchor="la", mono=False, alpha=255):
    """Text with the design's hard shadow, 3 px right and down."""
    f = font(size, mono)
    x, y = xy
    draw.text((x + 3, y + 3), s, font=f, fill=SHADOW + (alpha,), anchor=anchor)
    draw.text((x, y), s, font=f, fill=fill + (alpha,), anchor=anchor)


def bevel_box(draw, box, fill):
    """A bevelled cell: light top and left edges, dark bottom and right."""
    x0, y0, x1, y1 = box
    draw.rectangle(box, fill=fill)
    draw.rectangle((x0, y0, x1, y0 + 3), fill=BEVEL_LIGHT)
    draw.rectangle((x0, y0, x0 + 3, y1), fill=BEVEL_LIGHT)
    draw.rectangle((x0, y1 - 3, x1, y1), fill=BEVEL_DARK)
    draw.rectangle((x1 - 3, y0, x1, y1), fill=BEVEL_DARK)


def ochre_band(img, y0, y1):
    t = np.linspace(0.0, 1.0, y1 - y0)[:, None, None]
    top = np.array(OCHRE_TOP, dtype=float)
    bottom = np.array(OCHRE_BOTTOM, dtype=float)
    rows = (top + (bottom - top) * t).astype(np.uint8)
    img.paste(Image.fromarray(np.repeat(rows, W, axis=1), "RGB"), (0, y0))


def ease(t):
    """Ease out cubic on [0, 1]."""
    t = min(max(t, 0.0), 1.0)
    return 1 - (1 - t) ** 3


def fade(t, start, length):
    return min(max((t - start) / length, 0.0), 1.0)


# --- Audio ---------------------------------------------------------------


def seconds(n):
    return np.arange(int(n * RATE)) / RATE


def envelope(n_samples, attack, release):
    env = np.ones(n_samples)
    a, r = int(attack * RATE), int(release * RATE)
    if a:
        env[:a] = np.linspace(0, 1, a)
    if r:
        env[-r:] *= np.linspace(1, 0, r)
    return env


def pad(chord, length, seed):
    """A soft pad: detuned sines and a touch of second harmonic, slow tremolo."""
    rng = np.random.default_rng(seed)
    t = seconds(length)
    out = np.zeros_like(t)
    for f in chord:
        for detune in (-0.6, 0.0, 0.7):
            phase = rng.uniform(0, 2 * math.pi)
            out += np.sin(2 * math.pi * (f + detune) * t + phase)
            out += 0.15 * np.sin(4 * math.pi * (f + detune) * t + phase)
    out *= 1 + 0.15 * np.sin(2 * math.pi * 0.3 * t)
    noise = np.convolve(rng.normal(0, 1, t.size), np.ones(200) / 200, mode="same")
    out = out / np.abs(out).max() + 0.05 * noise
    return out * envelope(t.size, 1.8, 2.5)


def sting(length, pitch):
    """A rising sweep into a bright chord hit that decays."""
    t = seconds(length)
    sweep_len = 0.45
    sweep_t = t[t < sweep_len]
    freq = pitch * (1 + 5 * (sweep_t / sweep_len) ** 2)
    sweep = np.sin(2 * math.pi * np.cumsum(freq) / RATE) * np.linspace(0.1, 0.8, sweep_t.size)
    hit_t = t[t >= sweep_len] - sweep_len
    hit = np.zeros_like(hit_t)
    for mult in (2, 3, 4, 5, 6):
        hit += np.sin(2 * math.pi * pitch * mult * hit_t) / mult
    hit *= np.exp(-hit_t * 1.6)
    out = np.concatenate([sweep, hit])
    return out / np.abs(out).max() * envelope(t.size, 0.01, 0.4)


def brass(chords, beat):
    """A slow chorale: each chord `beat` seconds, brass-like harmonics."""
    parts = []
    for chord in chords:
        t = seconds(beat)
        tone = np.zeros_like(t)
        for f in chord:
            for h, amp in ((1, 1.0), (2, 0.5), (3, 0.3), (4, 0.15)):
                tone += amp * np.sin(2 * math.pi * f * h * t) * (1 + 0.02 * np.sin(2 * math.pi * 5 * t))
        tone *= envelope(t.size, 0.35, 0.5)
        parts.append(tone / np.abs(tone).max())
    return np.concatenate(parts)


def write_wav(path, samples):
    stereo = np.repeat((np.clip(samples, -1, 1) * 0.8 * 32767).astype(np.int16)[:, None], 2, axis=1)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(stereo.tobytes())


# --- Encoding --------------------------------------------------------------


def encode(path, frames, n_frames, audio):
    """Pipes `n_frames` RGB frames and the `audio` samples into ffmpeg."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        wav = Path(tmp) / "a.wav"
        write_wav(wav, audio)
        ff = subprocess.Popen(
            [
                "ffmpeg", "-loglevel", "error", "-y",
                "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
                "-i", str(wav),
                "-c:v", "libx264", "-profile:v", "high", "-pix_fmt", "yuv420p", "-preset", "medium", "-crf", "18",
                "-af", "loudnorm=I=-16:TP=-1.5:LRA=11", "-c:a", "aac", "-b:a", "192k", "-ar", "48000",
                "-movflags", "+faststart", "-shortest", str(path),
            ],
            stdin=subprocess.PIPE,
        )
        for i in range(n_frames):
            ff.stdin.write(frames(i / FPS).convert("RGB").tobytes())
        ff.stdin.close()
        if ff.wait() != 0:
            raise SystemExit(f"ffmpeg failed on {path}")
    print(path)


# --- Clips -----------------------------------------------------------------


def make_stinger(out, n, mark):
    """Three guide rows sweep in, staggered, then the mark lands."""
    length = 4.0
    colors = [SERIES, MOVIE, MUSIC]
    order = colors[n % 3:] + colors[: n % 3]
    from_left = n % 2 == 0
    base = field()
    row_h, gap = 150, 24
    top = (H - 3 * row_h - 2 * gap) // 2
    mark_big = mark.resize((360, 360), Image.LANCZOS)

    def frame(t):
        img = base.copy()
        d = ImageDraw.Draw(img, "RGBA")
        for i, color in enumerate(order):
            p = ease((t - 0.15 * i) / 0.6)
            y0 = top + i * (row_h + gap)
            width = int(W * p)
            x0 = 0 if from_left else W - width
            if width > 8:
                bevel_box(d, (x0, y0, x0 + width, y0 + row_h), color)
        land = ease((t - 0.9) / 0.5)
        if land > 0:
            d.rectangle((0, 0, W, H), fill=FIELD_BOTTOM + (int(170 * land),))
            size = int(360 * (0.6 + 0.4 * land))
            m = mark_big.resize((size, size), Image.LANCZOS)
            m.putalpha(m.getchannel("A").point(lambda a: int(a * land)))
            img.paste(m, ((W - size) // 2, (H - size) // 2 - 70), m)
            text(d, (W // 2, H // 2 + 200), "SCTV", 120, YELLOW, "mm", alpha=int(255 * land))
        out_fade = fade(t, length - 0.35, 0.35)
        if out_fade:
            d.rectangle((0, 0, W, H), fill=(0, 0, 0, int(255 * out_fade)))
        return img

    pitch = [220.0, 246.94, 196.0, 261.63, 174.61, 233.08][n % 6]
    encode(out / "stingers" / f"sctv-sting-{n + 1:02}.mp4", frame, int(length * FPS), sting(length, pitch))


def make_bump(out, n, lines, mark):
    """Lines of text fade in one after another; the SCTV band at the bottom."""
    length = 11.0
    base = field()
    d0 = ImageDraw.Draw(base)
    ochre_band(base, H - 120, H - 40)
    small = mark.resize((64, 64), Image.LANCZOS)
    base.paste(small, (96, H - 112), small)
    d0.text((180, H - 80), "SCTV", font=font(52), fill=OCHRE_INK, anchor="lm")
    d0.text((W - 96, H - 80), "BACK IN A MINUTE" if n % 2 else "STAY TUNED", font=font(40, mono=True),
            fill=OCHRE_INK, anchor="rm")
    size = 96 if max(len(s) for s in lines) < 30 else 80
    line_h = int(size * 1.35)
    y_top = (H - 140 - line_h * len(lines)) // 2

    def frame(t):
        img = base.copy()
        d = ImageDraw.Draw(img, "RGBA")
        for i, s in enumerate(lines):
            a = ease((t - 1.0 - 1.6 * i) / 0.8)
            if a > 0:
                text(d, (200, y_top + i * line_h), s, size, alpha=int(255 * a))
        f_in = 1 - fade(t, 0, 0.4)
        f_out = fade(t, length - 0.6, 0.6)
        black = max(f_in, f_out)
        if black:
            d.rectangle((0, 0, W, H), fill=(0, 0, 0, int(255 * black)))
        return img

    audio = pad(CHORDS[n % len(CHORDS)], length, seed=n)
    encode(out / "bumps" / f"sctv-bump-{n + 1:02}.mp4", frame, int(length * FPS), audio)


def make_ident(out, n, mark):
    """A calm slate that can be cut anywhere: the pad for short breaks."""
    length = 20.0
    base = field()
    d0 = ImageDraw.Draw(base)
    big = mark.resize((280, 280), Image.LANCZOS)
    base.paste(big, ((W - 280) // 2, 250), big)
    lines = [("We'll be right back", "SCTV"), ("Up next on SCTV", "STAY TUNED")][n % 2]
    text(d0, (W // 2, 640), lines[0], 96, WHITE, "mm")
    ochre_band(base, 760, 830)
    d0.text((W // 2, 795), lines[1], font=font(44, mono=True), fill=OCHRE_INK, anchor="mm")

    def frame(t):
        img = base.copy()
        d = ImageDraw.Draw(img, "RGBA")
        f_in = 1 - fade(t, 0, 0.3)
        if f_in:
            d.rectangle((0, 0, W, H), fill=(0, 0, 0, int(255 * f_in)))
        return img

    audio = pad(CHORDS[(n + 3) % len(CHORDS)], length, seed=100 + n) * 0.5
    encode(out / "idents" / f"sctv-slate-{n + 1:02}.mp4", frame, int(length * FPS), audio)


def color_bars():
    """SMPTE-style bars, from the stand-by card's geometry scaled to 1080p."""
    img = Image.new("RGB", (W, H))
    d = ImageDraw.Draw(img)
    top = [(0xC0, 0xC0, 0xC0), (0xC0, 0xC0, 0x00), (0x00, 0xC0, 0xC0), (0x00, 0xC0, 0x00),
           (0xC0, 0x00, 0xC0), (0xC0, 0x00, 0x00), (0x00, 0x00, 0xC0)]
    bw = W / 7
    for i, c in enumerate(top):
        d.rectangle((int(i * bw), 0, int((i + 1) * bw), int(H * 0.67)), fill=c)
    mid = [(0x00, 0x00, 0xC0), (0x13, 0x13, 0x13), (0xC0, 0x00, 0xC0), (0x13, 0x13, 0x13),
           (0x00, 0xC0, 0xC0), (0x13, 0x13, 0x13), (0xC0, 0xC0, 0xC0)]
    for i, c in enumerate(mid):
        d.rectangle((int(i * bw), int(H * 0.67), int((i + 1) * bw), int(H * 0.75)), fill=c)
    low = [((0x00, 0x21, 0x4C), 1.25), ((0xFF, 0xFF, 0xFF), 1.25), ((0x32, 0x00, 0x6A), 1.25),
           ((0x13, 0x13, 0x13), 3.25)]
    x = 0.0
    for c, span in low:
        d.rectangle((int(x * bw), int(H * 0.75), int((x + span) * bw), H), fill=c)
        x += span
    return img


def make_sign_off(out, mark, lockup):
    """Station card, closing cards over a chorale, then bars and tone."""
    cards = [
        ["This concludes", "our broadcast day."],
        ["SCTV is a service of", "SURFCULT."],
        ["Programming resumes", "at 6:00 a.m."],
        ["Thank you for watching.", "Good night."],
    ]
    card_len, intro, bars_len = 7.0, 6.0, 24.0
    length = intro + card_len * len(cards) + bars_len
    base = field()
    lock = lockup.resize((1100, int(1100 * lockup.height / lockup.width)), Image.LANCZOS)
    bars = color_bars()
    bd = ImageDraw.Draw(bars)
    bevel_box(bd, (W // 2 - 220, 300, W // 2 + 220, 420), CELL)
    bd.text((W // 2, 360), "SCTV", font=font(88), fill=YELLOW, anchor="mm")

    def frame(t):
        if t >= intro + card_len * len(cards):
            img = bars.copy()
            d = ImageDraw.Draw(img, "RGBA")
            f_out = fade(t, length - 2.0, 2.0)
            if f_out:
                d.rectangle((0, 0, W, H), fill=(0, 0, 0, int(255 * f_out)))
            return img
        img = base.copy()
        d = ImageDraw.Draw(img, "RGBA")
        if t < intro:
            a = ease(t / 1.5) * (1 - fade(t, intro - 0.8, 0.8))
            l2 = lock.copy()
            l2.putalpha(l2.getchannel("A").point(lambda v: int(v * a)))
            img.paste(l2, ((W - lock.width) // 2, (H - lock.height) // 2), l2)
        else:
            i = int((t - intro) // card_len)
            local = (t - intro) - i * card_len
            a = ease(local / 1.0) * (1 - fade(local, card_len - 0.8, 0.8))
            small = mark.resize((120, 120), Image.LANCZOS)
            img.paste(small, ((W - 120) // 2, 240), small)
            for j, s in enumerate(cards[i]):
                text(d, (W // 2, 520 + j * 130), s, 104, WHITE, "mm", alpha=int(255 * a))
        f_in = 1 - fade(t, 0, 0.5)
        if f_in:
            d.rectangle((0, 0, W, H), fill=(0, 0, 0, int(255 * f_in)))
        return img

    # Bb major chorale under the intro and the cards, then a 1 kHz tone.
    chords = [
        [116.54, 174.61, 233.08, 293.66], [155.56, 196.00, 233.08, 311.13],
        [174.61, 220.00, 261.63, 349.23], [116.54, 174.61, 233.08, 293.66],
        [196.00, 233.08, 293.66, 392.00], [155.56, 233.08, 311.13, 392.00],
        [174.61, 220.00, 261.63, 349.23], [116.54, 174.61, 233.08, 293.66],
    ]
    music_len = intro + card_len * len(cards)
    beat = music_len / len(chords)
    music = brass(chords, beat) * envelope(int(music_len * RATE), 1.0, 3.0)
    tone_t = seconds(bars_len)
    tone = 0.25 * np.sin(2 * math.pi * 1000 * tone_t) * envelope(tone_t.size, 0.05, 2.0)
    audio = np.concatenate([music, tone])
    encode(out / "sign-off" / "sctv-sign-off.mp4", frame, int(length * FPS), audio)


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "clips-out")
    mark = raster(str(ROOT / "assets/brand/mark.svg"), 720)
    lockup = raster(str(ROOT / "assets/brand/lockup-on-dark.svg"), 2200)
    jobs = [("sign-off", None)] + [("sting", n) for n in range(6)] + [("ident", n) for n in range(2)]
    jobs += [("bump", n) for n in range(len(BUMPS))]
    only = os.environ.get("ONLY")
    for kind, n in jobs:
        if only and kind != only:
            continue
        if kind == "sign-off":
            make_sign_off(out, mark, lockup)
        elif kind == "sting":
            make_stinger(out, n, mark)
        elif kind == "ident":
            make_ident(out, n, mark)
        else:
            make_bump(out, n, BUMPS[n], mark)


if __name__ == "__main__":
    main()
