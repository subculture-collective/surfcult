#!/usr/bin/env python3
"""Gap between the end of one file and the first picture of the next, with and without playlist prefetch."""
import json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mpvipc import Mpv, J
items = json.load(open(os.path.expanduser("~/spike/jf_items.json")))
url = lambda k: f"{J}/Videos/{items[k]['id']}/stream?static=true&mediaSourceId={items[k]['source']}"
PAIRS = [("hevc10 -> hevc8", "hevc10_1080_24", "hevc8_1080_24", 2400), ("hevc10 -> h264", "hevc10_1080_24", "h264_1080_24", 2400), ("hevc10 -> hevc10 (other film)", "hevc10_1080_24", "truehd_hevc", 2400)]
print(f"{'pair':32} {'prefetch':>8} {'gap s':>8}  notes")
for pf in ("no", "yes"):
    for name, a, b, off in PAIRS:
        m = Mpv(extra=[f"--prefetch-playlist={pf}", "--gapless-audio=yes"]); gap = None; note = ""
        try:
            m.cmd("loadfile", url(a), "replace", -1, {"pause": "yes"}); 
            for _ in range(100):
                d = m.prop("duration")
                if d: break
                time.sleep(0.1)
            m.cmd("seek", d - 25, "absolute"); m.cmd("set_property", "pause", False); m.wait_playing(30)
            m.cmd("loadfile", url(b), "append", -1, {"start": str(off)}); m.drain(); m.events.clear()
            t_end = None; t0 = time.monotonic()
            while time.monotonic() - t0 < 60:
                m.drain(0.02)
                for t, e in m.events:
                    if e.get("event") == "end-file" and t_end is None: t_end = t; note = e.get("reason", "")
                if t_end and m.prop("playlist-pos") == 0 and False: pass
                if t_end:
                    p = m.prop("path") or ""
                    if items[b]["id"] in p and m.prop("core-idle") is False and (m.prop("time-pos") or 0) >= off: gap = time.monotonic() - t_end; break
        finally: m.close()
        print(f"{name:32} {pf:>8} {('%.2f' % gap) if gap is not None else 'n/a':>8}  end-file={note}", flush=True)
