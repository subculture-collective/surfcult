# Read API

`surfcult run` serves the web admin and the API described here, with control of the TV; `surfcult serve <channels.toml> <db>` serves the same reads on their own, with no TV to control. The API gives channels, the schedule, the guide and health as JSON over HTTP, the guide as XMLTV, and the web admin page that reads them. The code is in `src/server/`. Every route but `POST /tune` and `POST /crt` is a fixed `GET`, and nothing runs a command, writes to the database or reads a file at run time; those two hand a request to the TV's run loop, which carries it out. Channels come from the parsed `channels.toml` and everything else from the SQLite store. Any other path gets `404` and any other method `405`.

## Settings

| Variable | Meaning |
| --- | --- |
| `SURFCULT_API_ADDR` | `ip:port` to listen on. Default `127.0.0.1:8090`, so only the Pi itself can connect. Set `0.0.0.0:8090` to allow the LAN, and set a token as well. |
| `SURFCULT_API_TOKEN` | A bearer token. When set, every request needs `Authorization: Bearer <token>` or gets `401` with `WWW-Authenticate: Bearer`. |
| `SURFCULT_API_TOKEN_FILE` | The path of a file holding the token, read when `SURFCULT_API_TOKEN` is unset. A file that cannot be read stops the server from starting rather than leaving it open. |

Without either token variable the server accepts every request. With one, the web admin's page and its files (below) are still served without it, as they hold no data; every read, and any unknown path, needs it.

The API is plain HTTP, with no TLS, so the bearer token and every answer cross the network unencrypted. Bind to an address other than localhost only on a trusted network, or put a TLS proxy in front of the server. The token is trimmed, must not contain whitespace or control characters, is compared in constant time and never appears in logs, errors or `Debug` output.

## Database access

The server opens its own read-only connection to the database file (`Store::open_read_only`). It does not create the file or migrate it: a `<db>` path that does not exist stops the server from starting, and the server never writes to the database. The program that writes the schedule creates and migrates the file. The file is in WAL mode, so these reads do not block the program that writes the schedule, and they see each committed write. Each request reads the store on a blocking thread.

## Endpoints

Times are Unix epoch milliseconds. Errors come back as `{"error": "..."}`.

### `GET /channels`

```json
{"channels": [{"number": 3, "name": "SATURDAY CARTOONS", "mode": "shuffle", "grid_ms": 1800000}]}
```

`mode` is `sequential`, `shuffle`, `weighted` or `blocks`. `grid_ms` is `null` for a channel without a grid.

### `GET /schedule?channel=&from=&to=`

Every parameter is optional. `channel` limits the answer to one channel; without it every configured channel is listed. `from` defaults to now and `to` to one day after `from`. The window `[from, to)` must be non-empty and at most seven days long; otherwise the answer is `400`. An unknown channel gets `404`.

An event that started before `from` but is still on at `from` is included, so the first event is what is on now.

```json
{"from_ms": 1000000, "to_ms": 87400000, "channels": [{"number": 3, "events": [
  {"start_ms": 940000, "end_ms": 1001000, "media_id": "a", "media_offset_ms": 0,
   "media_duration_ms": 60000, "kind": "program"}
]}]}
```

`kind` is `program`, `break`, `filler` or `ident`. Events carry media ids, not titles.

### `GET /guide?from=&to=`

Every configured channel's programs overlapping `[from, to)`, with titles: the guide's cells, as the TV's guide shows them, so a program cut by breaks is one cell and breaks, filler and idents are left out. `from` defaults to now and `to` to three hours after `from`; the window must be non-empty and at most a day, or the answer is `400`. `timezone` is the TV's, from `channels.toml`, so a client can show the times the TV shows.

```json
{"from_ms": 1000000, "to_ms": 11800000, "now_ms": 1000000, "timezone": "America/Chicago",
 "channels": [{"number": 3, "name": "SATURDAY CARTOONS", "cells": [
   {"start_ms": 940000, "end_ms": 3600000, "title": "The Simpsons: Bart the Genius", "movie": false, "genres": ["Animation"]}
 ]}]}
```

`title` is `Series: Episode` for an episode, the title for anything else, or `UNKNOWN PROGRAM` when the item has left the library. A cell's start and end are the program's own, not clipped to the window.

### `GET /xmltv.xml`

The guide as XMLTV (`application/xml`), for external guide clients. It lists every channel in `channels.toml` and the programs from now to the seven-day horizon (`from` is the request time). Programs are the guide's cells, not raw segments: a program cut by breaks is one `<programme>`, and breaks, filler and idents are left out. Times are `YYYYMMDDHHMMSS +0000`. Channel ids are `<number>.surfcult`.

```xml
<channel id="3.surfcult"><display-name>SATURDAY CARTOONS</display-name></channel>
<programme start="19700101001540 +0000" stop="19700101001641 +0000" channel="3.surfcult">
  <title>Tom &amp; Jerry</title>
  <category>Animation</category>
</programme>
```

`<title>` is the item's title, or its media id when the item has left the library. `<category>` repeats once per genre. There is no `<icon>` because the channel config has no logo field, and no `<desc>` because the media model has no description yet.

### `GET /health`

```json
{"status": "ok", "now_ms": 1000000, "schema_version": 3, "library_synced_at_ms": 42,
 "channels": [{"number": 3, "scheduled_until_ms": 605800000}]}
```

`library_synced_at_ms` is `null` while the library cache is empty. `scheduled_until_ms` is when the channel's last stored event ends, `null` if it has none; a value close to `now_ms` means the schedule is running out. If the database cannot be read, or its schema version is not the one this build expects (for example before the writer has run a new build's migrations), the answer is `503`.

## Controlling the TV

Inside `surfcult run`, the admin can change what the TV does. The run loop reads each request at its next step, within about 20 ms, and does what the remote or the Picture menu would. Under `surfcult serve` there is no TV: `/tv` says so and the two `POST`s answer `503`. Both need the token like every read. They take JSON only (`Content-Type: application/json`), so a plain form on another site cannot send one; a body that is not JSON gets `415`, and JSON of the wrong shape `422`.

### `GET /tv`

```json
{"controllable": true, "channel": 4, "crt": "heavy"}
```

`channel` is the channel tuned last (`null` before the first), and `crt` the CRT filter in use. Without a TV: `{"controllable": false, "channel": null, "crt": null}`.

### `POST /tune`

`{"channel": 5}` changes channel, as the remote would. A channel `channels.toml` does not have gets `404`. The answer is `202`: the change starts at the TV's next step.

### `POST /crt`

`{"level": "light"}` sets the CRT filter, as the Picture menu would, and stores it. `level` is `heavy`, `light` or `off`, in any case; anything else gets `400`. The answer is `202`.

## The web admin

`GET /` is the web admin: one page in the Cable Grid design ([cable-grid.md](../design/cable-grid.md#web-admin)), with its script, stylesheet, fonts and mark compiled into the binary (`assets/web/`, `assets/fonts/`, `assets/brand/`) and served at `/admin.js`, `/admin.css`, `/fonts/archivo-narrow-bold.ttf`, `/fonts/vcr-osd-mono.ttf`, `/mark.svg` and `/favicon.svg` with `Cache-Control: no-cache`. It is read only and draws everything in the browser from the reads above:

- **Guide** (`#guide`): every channel's next three hours from the current half hour, from `/guide`, with the program on now outlined in yellow and a line at the current time. At phone width (720 px or less) it becomes an "On now" list: each channel's program on now, a progress bar and what is next.
- **Channels** (`#channels`): each channel's number, name and mode from `/channels`, its program on now with progress, and the next one.
- **System** (`#system`): `/health` as the status, how long ago the library synced, the schema version and the time zone, and each channel's schedule end, flagged when it runs out within a day or is empty.

Times are in the TV's time zone. The page reads again every 30 seconds. When a read answers `401`, the page asks for the token and keeps it in the browser's local storage, sending it as `Authorization: Bearer` with every read.

When `/tv` says the TV can be controlled, the page adds a Tune button to each channel on the Channels view and in the On now list, marks the channel on the TV, and shows the CRT filter's three levels on the System view. After a request it says what it asked for and reads the page again. Channels themselves stay in `channels.toml`; the admin does not edit them.
