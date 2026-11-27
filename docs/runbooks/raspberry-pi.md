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

The Pi's CEC adapter works (`cec-ctl -d /dev/cec0`). Check it with `cec-ctl -d /dev/cec0 --playback --show-topology`: the TV should appear at logical address 0. On the first TV tried, HDMI-CEC was off in the TV's settings (the name varies by brand, for example Anynet+, Bravia Sync or Simplink) and the TV never answered. On the second TV it worked at once: the TV, an audio system at `1.0.0.0` and the Pi at `2.0.0.0` all showed.

Remote keys arrive as Linux input events from the input device named `vc4-hdmi-0`. Find it by that name: its number changed between boots (`event2`, then `event0`). Read it with `patrick`'s `input` group; no desktop session is needed. On 2026-11-23 these keys arrived: up, down, left, right, channel up, channel down and OK (reported as `KEY_OK`, code 352). On 2026-11-27 the TCL Fire TV remote sent Menu as `KEY_MENU` (139), Back as `KEY_EXIT` (174), Play/Pause alternating `KEY_PLAYCD` (200) and `KEY_PAUSECD` (201), Rewind `KEY_REWIND` (168) and Fast-forward `KEY_FASTFORWARD` (208). Home, volume and mute never reached the Pi, and the remote has no number keys. Earlier, number keys, Back, Menu and volume were not seen: they may not have been pressed, and volume normally goes to the audio system, not the Pi. If keys do not arrive, send `cec-ctl -d /dev/cec0 --active-source phys-addr=PHYSICAL_ADDRESS` so the TV treats the Pi as the source.

## Run the TV

`surfcult run` is the TV: remote, channel changes, schedules, MPV and the banner ([run reference](../reference/run.md)). [Boot into surfcult](#boot-into-surfcult) below makes the Pi start it at power-on. This section is for a manual run over SSH, for example to test a change: stop the service first (`sudo systemctl stop surfcult`), because nothing else may hold the display. Stop any MPV left from a test too.

1. Get the binary. The updater installs each `main` build as `/opt/surfcult/current/surfcult` (see Updates below). Without the updater, copy the `surfcult-aarch64` artifact of a `main` run to the Pi. Do not build on the Pi.
2. Put a `channels.toml` and the clip pools on the Pi. Every `[[interstitial_pool]]` path must exist and hold clips, at least the tune-in pool (`tune_in_pool`, default `tune-in`). The example config uses `/var/lib/surfcult/clips/<pool>`. Check the file with `surfcult check-config`.
3. Check the clock: `timedatectl show -p NTPSynchronized --value` must print `yes`, or schedules are not written.
4. Start it. The paths below are examples; the key file is the one from the playback checks.

```bash
ssh surfcult
export SURFCULT_JELLYFIN_URL=http://10.0.0.200:8096
export SURFCULT_JELLYFIN_API_KEY_FILE=$HOME/spike/.jf-key
# export SURFCULT_JELLYFIN_USER_ID=<user id>   # optional; else the first enabled user
/opt/surfcult/current/surfcult check-config ~/surfcult/channels.toml
/opt/surfcult/current/surfcult run ~/surfcult/channels.toml ~/surfcult/surfcult.db
```

On the first start the library cache is empty, so every channel is logged as unresolvable and shows the stand-by card until the first library sync is written; then the schedules are filled and the tuned channel starts playing. Logs go to the terminal, each line starting `surfcult:`.

Use the TV remote: up or channel up for the next channel, down or channel down for the previous one, OK for the banner; digits on a keyboard enter a channel number. The keys are configurable ([run reference](../reference/run.md#keys)). If no key arrives, see CEC and the remote above. Stop with Ctrl-C, or Q on a USB keyboard; both stop the transcodes and MPV. Closing the SSH session stops it too.

Check what the TV shows against the schedule from a second SSH session:

```bash
/opt/surfcult/current/surfcult now ~/surfcult/channels.toml ~/surfcult/surfcult.db
```

Each line has the channel, the program on now, its start and end, and the position the TV should be at.

## Boot into surfcult

`deploy/pi/surfcult.service` starts `surfcult run` at boot as `patrick`, with MPV on the TV. (The issue asked for `packaging/systemd/`; the Pi's other units are in `deploy/pi/`, so this one is there too.)

From a fresh Raspberry Pi OS Lite image (Debian 13, 64-bit) to a working TV:

1. Flash the image, set the user `patrick` with the operator's key, and boot. See [The device](#the-device) and [Reach it](#reach-it).
2. Install the packages in [What is installed](#what-is-installed): `sudo apt-get install mpv ffmpeg cec-utils socat jq` (and `git` to fetch this repository). Add `patrick` to the groups `video`, `render` and `input` (`audio` is already there): `sudo usermod -aG video,render,input patrick`. Log in again.
3. Put the Jellyfin key in a file only `patrick` can read, for example `~/spike/.jf-key` with mode 600. See [Play from Jellyfin](#play-from-jellyfin).
4. Put `channels.toml` and the clip pools in `~/surfcult/` and check it with `surfcult check-config` (see [Run the TV](#run-the-tv)). The service needs the binary, so do step 5 first if the updater is not yet running.
5. Install the service and the updater from a checkout of this repository: `cd deploy/pi && sudo ./install.sh`. Add the updater's token as described in [Updates](#updates-the-pi-pulls-the-newest-main-build), then `sudo systemctl start surfcult-updater` to fetch the first build.
6. Edit `/etc/surfcult/surfcult.env` if the Jellyfin address, the key file or the paths differ from the defaults. It is root-owned and world-readable, so it holds the key's path, never the key.
7. Start it: `sudo systemctl start surfcult`. Reboot once to prove the boot path: the TV should show the clock slate or the stand-by card within about a minute, then a channel.

What the unit does:

- Runs `/opt/surfcult/current/surfcult run` with the config and database from `surfcult.env`, in `~/surfcult`, as `patrick`, with the groups `video render input audio`. MPV's files are in `/run/surfcult/surfcult` (a runtime directory the unit creates), so they do not depend on `/run/user/1000` existing.
- Starts after `network-online.target` but does not wait for time sync. Until the clock is synchronized surfcult shows its own clock slate and writes no schedule ([the clock](../reference/run.md#schedules-and-the-clock)).
- `Restart=always` after 5 s, including after a clean exit. After 10 failed starts in 5 minutes it gives up and stays failed (a bad config or missing key); fix it, then `sudo systemctl reset-failed surfcult && sudo systemctl start surfcult`.
- Stops with SIGTERM, which surfcult handles: it stops the transcodes and MPV, then exits. Waiting for a library fetch can take up to 30 s, so the stop timeout is 45 s. `sudo systemctl stop surfcult` is therefore safe, and so are shutdown and reboot. The Q key on a USB keyboard also exits it, and the unit then restarts it after 5 s; stop the service to keep it down.

The console. surfcult draws with MPV straight on the display (KMS), and the USB keyboard that works as a remote is also the keyboard of tty1. `install.sh` therefore masks `getty@tty1.service`: no login prompt runs on tty1, so a stray keyboard cannot reach a shell on the TV. Masking, not just `Conflicts=` in the unit, because logind starts `getty@tty1` on its own when the console is switched to, which would stop surfcult. MPV does not need the console: it ran over plain SSH before, and the unit gives it no terminal. To get a console back:

- SSH in (the normal way to operate the Pi), or
- press Ctrl-Alt-F2 on a USB keyboard for a login on tty2. logind still starts it, so a keyboard on the Pi can reach a login. If that is not acceptable, set `NAutoVTs=0` in `/etc/systemd/logind.conf.d/` (not done by `install.sh`), or
- to restore tty1 fully: `sudo systemctl stop surfcult; sudo systemctl unmask getty@tty1.service; sudo systemctl start getty@tty1.service`.

Operate:

```bash
systemctl status surfcult                   # running or not, last exit
journalctl -u surfcult -f                   # logs; every line from surfcult starts "surfcult:"
journalctl -u surfcult -b -p warning        # this boot, warnings and worse
sudo systemctl restart surfcult
sudo systemctl disable --now surfcult       # no start at boot, stop now
sudo systemctl enable --now surfcult        # and back
```

Updates and rollback. The updater already restarts `surfcult.service` after it installs a build, waits 10 s and rolls back to `previous` if the unit is not active ([Updates](#updates-the-pi-pulls-the-newest-main-build)); it did nothing before because the unit did not exist, so no change to the updater was needed. A restart blanks the TV for a few seconds, and it is the price of the update. To avoid an update at a bad time, stop the timer. To go back by hand to the previous build: `sudo ln -sfn releases/$(basename $(readlink /opt/surfcult/previous)) /opt/surfcult/current && sudo systemctl restart surfcult`, and disable the timer so it does not reinstall the bad one. A build that failed on this Pi is not retried.

Not checked on the Pi yet (the unit is written but not applied): a cold boot into the TV, `systemctl stop` during a transcode, and an update while a program plays. The owner checks these, then closes issue #36.

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

## Run the hardware checks

`tests/pi.rs` holds the checks that need the Pi, its TV and the owner's Jellyfin: the watchdog (kill and hang MPV), both Jellyfin URL kinds playing, and the stand-by card's cost. They are `#[ignore]`d. Build the test binary on another machine (the Pi does not build Rust) and run it on the Pi. From a Linux x86_64 machine with `zig` and the Rust target:

```bash
rustup target add aarch64-unknown-linux-gnu
```

Make a linker wrapper, `zig-cc.sh`, that runs `zig cc` for the Pi and drops the two flags it does not take:

```sh
#!/bin/sh
args=""
for a in "$@"; do
  case "$a" in
    --target=*|-Wl,--fix-cortex-a53-843419) ;;
    *) args="$args '$(printf %s "$a" | sed "s/'/'\\\\''/g")'" ;;
  esac
done
eval exec zig cc -target aarch64-linux-gnu.2.36 $args
```

Then build, copy and run:

```bash
chmod +x zig-cc.sh
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=$PWD/zig-cc.sh CC_aarch64_unknown_linux_gnu=$PWD/zig-cc.sh AR_aarch64_unknown_linux_gnu="zig ar"
cargo test --release --test pi --no-run --target aarch64-unknown-linux-gnu   # prints the executable path
scp PATH-FROM-ABOVE surfcult:/tmp/pi-test
ssh surfcult 'cd /tmp && SURFCULT_JELLYFIN_URL=http://10.0.0.200:8096 SURFCULT_JELLYFIN_API_KEY_FILE=$HOME/spike/.jf-key ./pi-test --ignored --nocapture --test-threads=1'
```

Run them one at a time (`--test-threads=1`): they share the TV. The Jellyfin check reads the key from the file and asserts that it never reaches MPV's command line. `SURFCULT_JELLYFIN_USER_ID` picks the Jellyfin user playback is asked for; without it the first enabled user is used. Results of the first run are in `docs/research/2026-11-24-pi-hardware-checks.md`.

## Roll back

- Packages: `sudo apt-get remove mpv ffmpeg git cec-utils socat jq`, then `sudo apt-get autoremove`.
- Sudo rule: see above.
- Service: `sudo systemctl disable --now surfcult`, `sudo rm /etc/systemd/system/surfcult.service`, `sudo systemctl unmask getty@tty1.service`, `sudo systemctl daemon-reload`. Keep `/etc/surfcult/surfcult.env` if you will reinstall; otherwise `sudo rm -rf /etc/surfcult`.
- Updater: `sudo systemctl disable --now surfcult-updater.timer`, then `sudo rm -rf /usr/local/sbin/surfcult-update /etc/systemd/system/surfcult-updater.* /opt/surfcult /etc/surfcult-updater` and `sudo systemctl daemon-reload`. Revoke the token on GitHub.
- T3 and Tailscale: `t3 environment unlink surfcult` on the linking machine. On the Pi: `tailscale serve --https=443 off`, `tailscale serve --http=3775 off`, `t3 service uninstall`, `t3 uninstall`, `rm -rf ~/.t3 ~/.config/systemd/user/t3code.service.d`, `sudo loginctl disable-linger patrick`, `sudo tailscale logout`, `sudo apt-get remove tailscale nodejs npm`. Delete the `surfcult` machine in the Tailscale admin console.
- Test media: `rm -rf ~/spike` on the Pi. The default key file in `surfcult.env` is `~/spike/.jf-key`; move it first if the service is still in use.
