import json, os, select, socket, subprocess, time
J = "http://10.0.0.200:8096"; NL = chr(10).encode()
class Mpv:
    def __init__(self, extra=(), sock="/tmp/mpv-spike2.sock"):
        self.sock = sock; subprocess.run(["rm", "-f", sock])
        self.p = subprocess.Popen(["mpv", "--no-config", f"--include={os.path.expanduser('~/spike/jf.conf')}", "--idle=yes", "--force-window=yes", "--profile=fast", "--vo=gpu",
            "--gpu-context=drm", "--drm-mode=1920x1080@60", "--hwdec=drm,v4l2m2m", "--ao=null", "--keep-open=no", f"--input-ipc-server={sock}", "--msg-level=all=error", *extra],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(60):
            if os.path.exists(sock): break
            time.sleep(0.1)
        self.s = socket.socket(socket.AF_UNIX); self.s.connect(sock); self.buf = b""; self.rid = 0; self.events = []
    def _pump(self, t):
        if select.select([self.s], [], [], t)[0]:
            d = self.s.recv(1 << 20)
            if d: self.buf += d
    def cmd(self, *c, timeout=10):
        self.rid += 1; my = self.rid; self.s.sendall((json.dumps({"command": list(c), "request_id": my}) + chr(10)).encode()); end = time.monotonic() + timeout
        while time.monotonic() < end:
            while NL in self.buf:
                line, self.buf = self.buf.split(NL, 1)
                try: m = json.loads(line)
                except ValueError: continue
                if m.get("request_id") == my: return m
                if "event" in m: self.events.append((time.monotonic(), m))
            self._pump(0.05)
        return None
    def prop(self, n):
        m = self.cmd("get_property", n); return m["data"] if m and m.get("error") == "success" else None
    def drain(self, t=0.0):
        end = time.monotonic() + t
        while True:
            while NL in self.buf:
                line, self.buf = self.buf.split(NL, 1)
                try: m = json.loads(line)
                except ValueError: continue
                if "event" in m: self.events.append((time.monotonic(), m))
            if time.monotonic() >= end: return
            self._pump(max(0.0, min(0.05, end - time.monotonic())))
    def cpu(self, pid=None): return sum(int(x) for x in open(f"/proc/{pid or self.p.pid}/stat").read().split()[13:15]) / os.sysconf("SC_CLK_TCK")
    def wait_playing(self, timeout=60):
        t0 = time.monotonic()
        while time.monotonic() - t0 < timeout:
            if self.prop("vo-configured") and self.prop("core-idle") is False and (self.prop("time-pos") or 0) >= 0: return time.monotonic() - t0
            time.sleep(0.03)
        return None
    def close(self):
        try: self.cmd("quit", timeout=2)
        except Exception: pass
        try: self.p.wait(timeout=8)
        except Exception: self.p.kill()
