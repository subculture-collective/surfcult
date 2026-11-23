import json, sys, urllib.request, collections
key = open(sys.argv[1]).read().strip(); J = "http://10.0.0.200:8096"
def get(path):
    r = urllib.request.Request(J + path, headers={"Authorization": f'MediaBrowser Token="{key}"'})
    return json.load(urllib.request.urlopen(r, timeout=60))
print("libraries:", [(v["Name"], v.get("CollectionType")) for v in get("/Library/VirtualFolders")])
print("counts:", {k: v for k, v in get("/Items/Counts").items() if v})
C = collections.defaultdict(collections.Counter); n = 0; start = 0; nov = 0
while True:
    d = get(f"/Items?Recursive=true&IncludeItemTypes=Movie,Episode&Fields=MediaStreams&Limit=500&StartIndex={start}&EnableTotalRecordCount=true")
    items = d["Items"]
    if not items: break
    for it in items:
        n += 1
        vs = [s for s in it.get("MediaStreams", []) if s["Type"] == "Video"]
        if not vs: nov += 1; continue
        v = vs[0]; kind = it["Type"]
        h = v.get("Height", 0); w = v.get("Width", 0)
        res = "4K" if h >= 2000 or w >= 3000 else "1080p" if h >= 1000 or w >= 1800 else "720p" if h >= 700 else "SD/other"
        fps = v.get("RealFrameRate") or v.get("AverageFrameRate") or 0
        fb = "<=25" if fps <= 25.5 else "29.97-30" if fps <= 30.5 else "48-50" if fps <= 50.5 else ">50"
        aud = [s for s in it.get("MediaStreams", []) if s["Type"] == "Audio"]
        ad = next((s for s in aud if s.get("IsDefault")), aud[0] if aud else {})
        for name, val in (("video codec", v.get("Codec")), ("resolution", res), ("bit depth", v.get("BitDepth")),
                          ("range", v.get("VideoRangeType")), ("frame rate", fb), ("interlaced", bool(v.get("IsInterlaced"))),
                          ("default audio codec", ad.get("Codec")), ("container", it.get("Container"))):
            C[name][val] += 1
        C["codec+depth"][f'{v.get("Codec")} {v.get("BitDepth")}-bit'] += 1
    start += 500
    if start >= d.get("TotalRecordCount", 0): break
print("video items:", n, "(without a video stream:", nov, ")")
for name, c in C.items():
    tot = sum(c.values()); print(f"\n{name}:"); [print(f"  {str(k):>14}: {v:>5} ({100*v/tot:.0f}%)") for k, v in c.most_common(9)]
