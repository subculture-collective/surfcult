# Raspberry Pi 4B (the surfcult TV box)

Purpose: reach the Pi, know what is installed on it, and play video on its TV the way surfcult will.

Observed 2026-11-22. Live state overrides this page.

## The device

| Item | Value |
| --- | --- |
| Hostname | `surfcult` |
| Hardware | Raspberry Pi 4 Model B, 2 GB (1.8 GiB usable), 128 GB microSD, HDMI to a 4K TV |
| OS | Raspberry Pi OS (Debian 13 trixie), Lite, no desktop, `multi-user.target` |
| Wired address | `10.0.0.53` (Volga DHCP reservation, 1 Gbps; the Pi 4B port cannot go faster) |
| Wi-Fi address | `10.0.0.54` (Volga DHCP reservation; backup path) |
| SSH user | `patrick` |
| SSH key | `~/.ssh/id_ed25519` on the operator's machine |
| SSH host key | `SHA256:IgxEOAh0rjwSI3R3zltUuqz3gOnISidsbygwaIqwSaU` (ed25519) |
| Tailscale | `100.108.254.88`, `surfcult.mooneye-mintaka.ts.net` (owner `onnwee@`; the address can change if the device is removed and rejoined) |
| Estate inventory | `surfcult` in `estate-config` `inventory/hosts.yaml` |

DHCP is owned by Volga (`10.0.0.1`). Both reservations are keyed by the interface MACs, which are not recorded here. Wired wins when both are up (default route metric 100 against 600).

## Reach it

```bash
ssh -i ~/.ssh/id_ed25519 -o IdentitiesOnly=yes patrick@10.0.0.53
```

An alias with a pinned host key avoids retyping this. Put the key line in `~/.ssh/fleet_known_hosts` and this block in `~/.ssh/config`:

```
Host surfcult
  HostName 10.0.0.53
  User patrick
  IdentityFile ~/.ssh/id_ed25519
  IdentitiesOnly yes
  HostKeyAlias fleet-surfcult
  StrictHostKeyChecking yes
  UserKnownHostsFile ~/.ssh/fleet_known_hosts ~/.ssh/known_hosts
  ConnectTimeout 5
```

If the login is refused with "Permission denied (publickey)", the user or key is wrong. The Pi takes keys only. The account is `patrick`, not `onnwee` or `pi`, and the accepted key is `id_ed25519`, not `id_ed25519_onnwee`.

## What is installed

- Installed: `mpv` 0.40, `ffmpeg` 7.1, `git`, `cec-utils`, `socat`, `jq`, plus the image's `v4l-utils`.
- Also installed: Tailscale 1.104.7, Node.js 20.19.2 and npm (from Debian), and the T3 CLI in `~/.local` (see below).
- Not installed on purpose: Rust (CI builds the aarch64 binary), Docker, mise, and the agent CLIs (Codex, Claude). T3 on the Pi therefore logs agent health-check warnings.
- `patrick` has passwordless sudo through `/etc/sudoers.d/010-patrick-nopasswd`, added 2026-11-22 at the owner's request. Remove it with `sudo rm /etc/sudoers.d/010-patrick-nopasswd`.
- `patrick` is in the `video`, `render` and `input` groups, so MPV can use the display and the remote without a desktop session.

## Play video the way surfcult will

Run over SSH, with no desktop. MPV draws straight to the TV through KMS.

```bash
mpv --profile=fast --vo=gpu --gpu-context=drm --drm-mode=1920x1080@60 --hwdec=drm FILE
```

- `--profile=fast` is required. MPV's default video settings overload the Pi's GPU and drop about a third of the frames, even with hardware decode.
- `--hwdec=drm` uses the Pi's H.265 decoder, including 10-bit, and hands frames to the display without copying them. For H.264 use `--hwdec=v4l2m2m`.
- Content at 24 fps plays cleanly. 60 fps content is not reliable. See [the playback spike](../research/2026-11-22-pi-playback-spike.md).
- The TV runs at 1920x1080 at 60 Hz. MPV's JSON IPC works with `--input-ipc-server=/tmp/mpv.sock` and `socat`. Use `--hwdec=drm,v4l2m2m` when the codec is not known in advance.

## Play from Jellyfin

Jellyfin 12.4.0 runs on Almaz at `http://10.0.0.200:8096` (LAN, plain HTTP). The public name is `jellyfin.subcult.tv`.

- Send the API key as `Authorization: MediaBrowser Token="KEY"`. Jellyfin 12 returns 401 for `X-Emby-Token` and `api_key`.
- On the Pi the key is in `~/spike/.jf-key` (mode 600). `~/spike/jf.conf` (mode 600) holds the header for MPV, so the key stays out of the process list: `mpv --include=$HOME/spike/jf.conf ...`. Never put the key on a command line, in a URL or in the repository.
- Direct play: `/Videos/{id}/stream?static=true&mediaSourceId={source}`.
- Transcode (AV1, H.264 10-bit, 4K, HDR): ask for a progressive stream, `/Videos/{id}/stream.ts` with `VideoCodec=h264`, `AudioCodec=aac`, `VideoBitrate`, `maxWidth`, `maxHeight`, `StartTimeTicks` and a new `PlaySessionId`. Do not use HLS at an offset; it takes about 19 s to start.

The test scripts are in `tools/pi-spike/`. Copy them to `~/spike/` on the Pi to run them.

## CEC and the remote

The Pi's CEC adapter works (`cec-ctl -d /dev/cec0`), but the TV does not answer it. Turn on HDMI-CEC in the TV's settings (the name varies by brand, for example Anynet+, Bravia Sync or Simplink) and rerun `cec-ctl -d /dev/cec0 --playback --show-topology`; the TV should appear at logical address 0. Remote keys then arrive as input events on `/dev/input/event2`.

## Updates: the Pi pulls the newest main build

GitHub's job builds the aarch64 binary on every push to `main` and keeps it as the `surfcult-aarch64` artifact for 90 days, with its SHA-256. A timer on the Pi (`surfcult-updater.timer`, every 10 minutes) runs `/usr/local/sbin/surfcult-update`, which:

1. asks GitHub for the newest successful push run of `pr-ci.yml` on `main`, and stops if that commit is already installed or failed on this Pi before;
2. downloads the artifact (the token goes to GitHub only, not to the signed download URL) and checks the file list, the checksum and that it is a 64-bit aarch64 ELF;
3. runs it once as the `nobody` user as a smoke test, then installs it as `/opt/surfcult/releases/<commit>/surfcult` and points `/opt/surfcult/current` at it, keeping the old target as `previous`;
4. restarts `surfcult.service` if that unit exists, and rolls back to `previous` if the unit is not active afterwards. A build that failed is listed in `/opt/surfcult/failed` and not retried.

It keeps the newest three releases. Nothing is built on the Pi.

Install or update the updater from a checkout of this repository (`deploy/pi/`): `sudo ./install.sh`. It does nothing until a token exists.

The token must be read-only and limited to this repository: a fine-grained GitHub token with only `Actions: read` and `Contents: read` on `subculture-collective/surfcult`, and an expiry. GitHub serves Actions artifacts only to signed-in callers, even for a public repository. Put it on the Pi without putting it on a command line:

```bash
ssh surfcult 'sudo install -m 600 -o root -g root /dev/stdin /etc/surfcult-updater/token' < token-file
```

Do not reuse a broader token. A classic token that can read private repositories or write is not acceptable on this device. Rotate by replacing the file; the next run uses it.

Operate:

```bash
sudo systemctl start surfcult-updater      # check now
journalctl -u surfcult-updater -n 20       # what it did
ls -l /opt/surfcult/current                # which commit is installed
```

To pin a build and stop updates: `sudo systemctl disable --now surfcult-updater.timer`. To go back by hand: `sudo ln -sfn releases/<commit> /opt/surfcult/current` and restart the service. The tests are `make updater-test`.

The smoke test runs the binary with no arguments and expects its usage text. Change it when the binary gains a `--version` or a health command.

## Tailscale and the T3 server

Tailscale joined with `sudo tailscale up --accept-dns=false --operator=patrick --hostname=surfcult`, the flags the other headless hosts use. Subnet routes are not accepted, so the Pi's LAN routing is untouched. Because MagicDNS is not used, a machine that wants the name `surfcult.mooneye-mintaka.ts.net` needs an `/etc/hosts` line for `100.108.254.88`.

T3 runs as the user service `t3code.service` for `patrick` (lingering is on, so it survives logout). It listens on `127.0.0.1:3773` only. Tailscale Serve proxies HTTPS 443 and HTTP 3775 to it, bound to the tailnet addresses, so nothing answers on `10.0.0.53`.

The Pi has 1.8 GiB of RAM and its job is video, so T3 is set to yield. The drop-in `~/.config/systemd/user/t3code.service.d/10-surfcult.conf` sets `CPUWeight=20`, `IOWeight=20`, `MemoryLow=0` and `OOMScoreAdjust=500`. T3's own unit asks for `MemoryLow=1536M`, and `t3 service protect` shields T3 from agents under memory pressure; neither suits this device, so do not apply them. T3 uses about 410 MB resident.

Update T3 with `t3 update` on the Pi, or reinstall with `npm install -g --prefix ~/.local t3@VERSION` and `t3 service install`. Use the version the rest of the fleet runs. Check it with:

```bash
ssh surfcult 'export PATH=$HOME/.local/bin:$PATH; t3 --version; systemctl --user is-active t3code.service; tailscale serve status'
curl -s -o /dev/null -w '%{http_code}\n' --resolve surfcult.mooneye-mintaka.ts.net:443:100.108.254.88 https://surfcult.mooneye-mintaka.ts.net/api/orchestration/shell   # 401 without credentials
```

Another T3 environment links to it with a pairing code from `t3 pair --ttl 10m` on the Pi, passed over stdin: `printf '%s %s\n' https://surfcult.mooneye-mintaka.ts.net CODE | t3 environment link --from - --access full-access`. Linked environments must be on (`linkedEnvironmentsEnabled` in `~/.t3/userdata/settings.json`). Revoke a link under Settings, Sharing and access, or with `t3 environment unlink surfcult` on the linking machine. A pairing code is a credential: keep it out of command lines and logs.

## Verify

```bash
ssh surfcult 'hostname; ip -br a; mpv --version | head -1; vcgencmd get_throttled; cec-client -l | head -3'
```

Expected: hostname `surfcult`, `eth0` on `10.0.0.53/24`, MPV 0.40, `throttled=0x0`, and a CEC adapter at `/dev/cec0`.

## Roll back

- Packages: `sudo apt-get remove mpv ffmpeg git cec-utils socat jq`, then `sudo apt-get autoremove`.
- Sudo rule: see above.
- Updater: `sudo systemctl disable --now surfcult-updater.timer`, then `sudo rm -rf /usr/local/sbin/surfcult-update /etc/systemd/system/surfcult-updater.* /opt/surfcult /etc/surfcult-updater` and `sudo systemctl daemon-reload`. Revoke the token on GitHub.
- T3 and Tailscale: `t3 environment unlink surfcult` on the linking machine. On the Pi: `tailscale serve --https=443 off`, `tailscale serve --http=3775 off`, `t3 service uninstall`, `t3 uninstall`, `rm -rf ~/.t3 ~/.config/systemd/user/t3code.service.d`, `sudo loginctl disable-linger patrick`, `sudo tailscale logout`, `sudo apt-get remove tailscale nodejs npm`. Delete the `surfcult` machine in the Tailscale admin console.
- Test media: `rm -rf ~/spike` on the Pi.
