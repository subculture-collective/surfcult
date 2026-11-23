#!/usr/bin/env bash
# usage: playtest.sh FILE HWDEC [SECONDS]   -> plays to the TV via DRM, samples mpv over its JSON IPC socket
f=$1; hw=$2; secs=${3:-20}; sock=/tmp/mpv-spike.sock; rm -f $sock
mpv --no-config --vo=gpu --gpu-context=drm --drm-mode=${MODE:-preferred} --hwdec=$hw --ao=${AO:-null} ${EXTRA:-} --length=$((secs+6)) --input-ipc-server=$sock "$f" >/tmp/mpv-spike.log 2>&1 &
pid=$!; sleep 5
q(){ echo "{\"command\":[\"get_property\",\"$1\"]}" | socat - $sock 2>/dev/null | jq -r '.data // "n/a"'; }
vd0=$(q frame-drop-count); dd0=$(q decoder-frame-drop-count); t0=$(awk '{print $14+$15}' /proc/$pid/stat); w0=$(date +%s.%N); sleep $secs
t1=$(awk '{print $14+$15}' /proc/$pid/stat); w1=$(date +%s.%N)
hwc=$(q hwdec-current); dd=$(q decoder-frame-drop-count); vd=$(q frame-drop-count); av=$(q avsync); fps=$(q estimated-vf-fps); vf=$(q video-format); pf=$(q video-params/pixelformat)
cores=$(awk -v a=$t0 -v b=$t1 -v c=$w0 -v d=$w1 'BEGIN{printf "%.2f",((b-a)/100)/(d-c)}')
echo "$(basename $f) | fmt=$vf $pf | hwdec=$hwc | dec_drops=$((dd-dd0)) vo_drops=$((vd-vd0)) (startup+window: $vd) | avsync=$av s | fps=$fps | cpu=${cores} cores"
kill $pid 2>/dev/null; wait $pid 2>/dev/null; grep -iE "software decoding|fail|error" /tmp/mpv-spike.log | head -3
