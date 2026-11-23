import json, sys, urllib.request
key = open(sys.argv[1]).read().strip(); J = "http://10.0.0.200:8096"
def get(p):
    return json.load(urllib.request.urlopen(urllib.request.Request(J+p, headers={"Authorization": f'MediaBrowser Token="{key}"'}), timeout=60))
want = {"hevc10_1080_24": lambda v,a,fps,h: v["Codec"]=="hevc" and v.get("BitDepth")==10 and h>=1000 and fps<=25.5 and v.get("VideoRangeType")=="SDR",
        "hevc8_1080_24":  lambda v,a,fps,h: v["Codec"]=="hevc" and v.get("BitDepth")==8 and h>=1000 and fps<=25.5,
        "h264_1080_24":   lambda v,a,fps,h: v["Codec"]=="h264" and v.get("BitDepth")==8 and h>=1000 and fps<=25.5,
        "hevc10_1080_30": lambda v,a,fps,h: v["Codec"]=="hevc" and v.get("BitDepth")==10 and h>=1000 and 29<=fps<=30.5,
        "av1_1080_24":    lambda v,a,fps,h: v["Codec"]=="av1" and h>=1000 and fps<=25.5,
        "hevc_4k_hdr":    lambda v,a,fps,h: v["Codec"]=="hevc" and h>=2000 and v.get("VideoRangeType")!="SDR",
        "truehd_hevc":    lambda v,a,fps,h: v["Codec"]=="hevc" and a.get("Codec")=="truehd" and h>=1000}
found = {}; start = 0
while len(found) < len(want):
    d = get(f"/Items?Recursive=true&IncludeItemTypes=Movie&Fields=MediaStreams,MediaSources&Limit=300&StartIndex={start}&SortBy=Random")
    if not d["Items"]: break
    for it in d["Items"]:
        if (it.get("RunTimeTicks") or 0) < 90*60*10**7: continue
        vs = [s for s in it.get("MediaStreams",[]) if s["Type"]=="Video"]
        if not vs: continue
        v = vs[0]; aud=[s for s in it["MediaStreams"] if s["Type"]=="Audio"]; a = next((s for s in aud if s.get("IsDefault")), aud[0] if aud else {})
        fps = v.get("RealFrameRate") or v.get("AverageFrameRate") or 0; h = v.get("Height",0)
        for k,f in want.items():
            if k not in found and f(v,a,fps,h):
                found[k] = {"id": it["Id"], "source": it["MediaSources"][0]["Id"], "video": f'{v["Codec"]} {v.get("BitDepth")}-bit {v.get("Width")}x{h} {fps:g}fps {v.get("VideoRangeType")}',
                            "audio": f'{a.get("Codec")} {a.get("Channels")}ch', "minutes": round(it["RunTimeTicks"]/6e8), "container": it.get("Container"), "mbps": round((it["MediaSources"][0].get("Bitrate") or 0)/1e6,1)}
    start += 300
    if start > 1500: break
json.dump(found, open(sys.argv[2],"w"), indent=1)
for k,v in found.items(): print(f"{k:16} {v['video']:42} audio={v['audio']:9} {v['minutes']}min {v['container']} {v['mbps']}Mbps")
print("missing:", [k for k in want if k not in found])
