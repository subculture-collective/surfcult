#!/usr/bin/env python3
"""Phase 0 tune-time harness. One idle mpv process, streams loaded over its JSON IPC socket."""
import json, os, socket, subprocess, sys, time
J = "http://10.0.0.200:8096"; SOCK = "/tmp/mpv-tune.sock"
items = json.load(open(os.path.expanduser("~/spike/jf_items.json")))
def direct(k): i = items[k]; return f"{J}/Videos/{i['id']}/stream?static=true&mediaSourceId={i['source']}"
def hls(k, kbps=8000, ticks=0):
    i = items[k]
    return (f"{J}/Videos/{i['id']}/master.m3u8?MediaSourceId={i['source']}&VideoCodec=h264&AudioCodec=aac&TranscodingMaxAudioChannels=2"
            f"&SegmentContainer=ts&MinSegments=1&BreakOnNonKeyFrames=true&VideoBitrate={kbps*1000}&AudioBitrate=192000&MaxStreamingBitrate={kbps*1000+192000}"
            f"&h264-profile=high&h264-level=41&maxWidth=1920&maxHeight=1080&StartTimeTicks={ticks}&PlaySessionId=spike-{k}-{int(time.time()*1000)}")
def prog(k, off=0, kbps=8000):
    i = items[k]
    return (f"{J}/Videos/{i['id']}/stream.ts?MediaSourceId={i['source']}&VideoCodec=h264&AudioCodec=aac&TranscodingMaxAudioChannels=2&VideoBitrate={kbps*1000}"
            f"&AudioBitrate=192000&h264-profile=high&h264-level=41&maxWidth=1920&maxHeight=1080&StartTimeTicks={off*10**7}&TranscodeReasons=VideoCodecNotSupported"
            f"&PlaySessionId=spike-p-{k}-{int(time.time()*1000)}")
mid = lambda k: int(items[k]["minutes"] * 60 * 0.4)
TESTS = [("direct hevc10 start=0", direct("hevc10_1080_24"), 0, 3), ("direct hevc10 start=40%", direct("hevc10_1080_24"), mid("hevc10_1080_24"), 3),
         ("direct hevc8 start=40%", direct("hevc8_1080_24"), mid("hevc8_1080_24"), 2), ("direct h264 start=40%", direct("h264_1080_24"), mid("h264_1080_24"), 2),
         ("direct hevc 4K HDR10 start=40%", direct("hevc_4k_hdr"), mid("hevc_4k_hdr"), 2), ("direct hevc10+truehd start=40%", direct("truehd_hevc"), mid("truehd_hevc"), 2),
         ("direct av1 10-bit start=40%", direct("av1_1080_24"), mid("av1_1080_24"), 2),
         ("direct h264 start=0", direct("h264_1080_24"), 0, 2),
         ("TRANSCODE av1->h264 8M start=0", lambda: hls("av1_1080_24"), 0, 3), ("TRANSCODE av1->h264 8M start=40%", lambda: hls("av1_1080_24"), mid("av1_1080_24"), 3),
         ("TRANSCODE hevc4k->h264 8M start=40%", lambda: hls("hevc_4k_hdr"), mid("hevc_4k_hdr"), 2),
         ("PROG-TS av1->h264 8M start=0", lambda: prog("av1_1080_24"), 0, 3), ("PROG-TS av1->h264 8M offset=40%", lambda: prog("av1_1080_24", mid("av1_1080_24")), 0, 3),
         ("PROG-TS hevc4k->h264 8M offset=40%", lambda: prog("hevc_4k_hdr", mid("hevc_4k_hdr")), 0, 3),
         ("PROG-TS hevc10->h264 8M offset=40%", lambda: prog("hevc10_1080_24", mid("hevc10_1080_24")), 0, 3)]
only = sys.argv[1:] 
subprocess.run(["rm", "-f", SOCK])
p = subprocess.Popen(["mpv", "--no-config", f"--include={os.path.expanduser('~/spike/jf.conf')}", "--idle=yes", "--force-window=yes", "--profile=fast", "--vo=gpu", "--gpu-context=drm",
                     "--drm-mode=1920x1080@60", "--hwdec=drm,v4l2m2m", "--ao=null", "--keep-open=no", f"--input-ipc-server={SOCK}", "--msg-level=all=error"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(50):
    if os.path.exists(SOCK): break
    time.sleep(0.1)
import select
s = socket.socket(socket.AF_UNIX); s.connect(SOCK); buf = b""; NL = chr(10).encode(); rid = [0]
def pump(timeout):
    global buf
    if select.select([s], [], [], timeout)[0]:
        d = s.recv(65536)
        if d: buf += d
def msgs():
    global buf
    while NL in buf:
        line, buf = buf.split(NL, 1)
        try: yield json.loads(line)
        except ValueError: pass
def cmd(*c, timeout=5):
    rid[0] += 1; my = rid[0]
    s.sendall((json.dumps({"command": list(c), "request_id": my}) + chr(10)).encode())
    end = time.monotonic() + timeout; seen = []
    while time.monotonic() < end:
        for m in msgs():
            if m.get("request_id") == my: return m, seen
            if "event" in m: seen.append(m)
        pump(0.05)
    return None, seen
def prop(n):
    m, _ = cmd("get_property", n); return m["data"] if m and m.get("error") == "success" else None
def cpu(): return sum(int(x) for x in open(f"/proc/{p.pid}/stat").read().split()[13:15]) / os.sysconf("SC_CLK_TCK")
print(f"{'test':34} {'time to picture s':>20}  {'hwdec':10} {'video':16} {'drops dec/vo':>12} {'cpu cores':>9}")
for name, url, start, reps in TESTS:
    if only and not any(o in name for o in only): continue
    tunes = []; last = {}
    for r in range(reps):
        cmd("stop"); time.sleep(1.0)
        opts = {"start": str(start)} if start else {}
        u = url() if callable(url) else url
        t0 = time.monotonic(); m, ev = cmd("loadfile", u, "replace", -1, opts)
        ok = False; fail = None
        while time.monotonic() - t0 < 90:
            vc = prop("vo-configured"); ci = prop("core-idle")
            if os.environ.get("DEBUG"): print(f"  t={time.monotonic()-t0:5.2f} vo-configured={vc} core-idle={ci} path={prop('path') is not None}", flush=True)
            if vc and ci is False: ok = True; break
            for e in msgs():
                if e.get("event") == "end-file" and e.get("reason") == "error": fail = e.get("file_error") or "error"
            if fail: break
            time.sleep(0.03)
        t1 = time.monotonic()
        if not ok: tunes.append(f"FAIL({fail or 'timeout'})"); continue
        tunes.append(round(t1 - t0, 2))
        c0, w0 = cpu(), time.monotonic(); d0 = prop("decoder-frame-drop-count") or 0; v0 = prop("frame-drop-count") or 0; time.sleep(6)
        c1, w1 = cpu(), time.monotonic()
        last = {"hw": prop("hwdec-current"), "fmt": f"{prop('video-format')} {prop('video-params/pixelformat') or ''}", "dd": (prop("decoder-frame-drop-count") or 0) - d0,
                "vd": (prop("frame-drop-count") or 0) - v0, "cpu": (c1 - c0) / (w1 - w0)}
    ts = "/".join(str(t) for t in tunes)
    print(f"{name:34} {ts:>20}  {str(last.get('hw')):10} {last.get('fmt','')[:16]:16} {str(last.get('dd',''))+'/'+str(last.get('vd','')):>12} {last.get('cpu',0):9.2f}", flush=True)
cmd("quit"); p.wait(timeout=10)
