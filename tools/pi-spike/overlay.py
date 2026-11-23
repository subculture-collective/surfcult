#!/usr/bin/env python3
"""Guide-overlay cost test: a full-screen BGRA bitmap over 24 fps hardware-decoded video, scrolled at several update rates."""
import json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mpvipc import Mpv, J
items = json.load(open(os.path.expanduser("~/spike/jf_items.json"))); it = items["hevc10_1080_24"]
W, H, EXTRA = 1920, 1080, 1200; stride = W * 4; path = "/dev/shm/guide.bgra"
b = bytearray(os.urandom(stride * (H + EXTRA))); b[3::4] = bytes([200]) * (len(b) // 4); open(path, "wb").write(b); del b
m = Mpv(); url = f"{J}/Videos/{it['id']}/stream?static=true&mediaSourceId={it['source']}"
m.cmd("loadfile", url, "replace", -1, {"start": "1800"}); print("tune:", round(m.wait_playing(), 2), "s", flush=True); time.sleep(3)
print(f"{'scenario':40} {'mpv cores':>9} {'script cores':>12} {'updates/s':>10} {'upd ms (avg/max)':>17} {'vo drops':>9}")
def run(name, rate, secs=12):
    d0 = m.prop("frame-drop-count") or 0; c0 = m.cpu(); s0 = time.process_time(); w0 = time.monotonic(); n = 0; lat = []; row = 0
    if rate is None: time.sleep(secs)
    else:
        if rate == 0: m.cmd("overlay-add", 0, 0, 0, path, 0, "bgra", W, H, stride); time.sleep(secs)
        else:
            nxt = w0
            while time.monotonic() - w0 < secs:
                t = time.monotonic(); m.cmd("overlay-add", 0, 0, 0, path, (row % EXTRA) * stride, "bgra", W, H, stride); lat.append((time.monotonic() - t) * 1000); n += 1; row += 6
                nxt += 1.0 / rate; sl = nxt - time.monotonic()
                if sl > 0: time.sleep(sl)
    w1 = time.monotonic(); c1 = m.cpu(); s1 = time.process_time(); d1 = m.prop("frame-drop-count") or 0
    ups = n / (w1 - w0) if n else 0; la = f"{sum(lat)/len(lat):.0f}/{max(lat):.0f}" if lat else "-"
    print(f"{name:40} {(c1-c0)/(w1-w0):9.2f} {(s1-s0)/(w1-w0):12.2f} {ups:10.1f} {la:>17} {d1-d0:9d}", flush=True)
    m.cmd("overlay-remove", 0)
run("no overlay (baseline)", None); run("static full-screen overlay", 0); run("scroll 10 updates/s", 10); run("scroll 30 updates/s", 30); run("scroll 60 updates/s (target)", 60)
m.close()
