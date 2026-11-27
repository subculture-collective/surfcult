# Read API

`surfcult serve <channels.toml> <db>` serves channels, the schedule and health as JSON over HTTP, and the guide as XMLTV. The code is in `src/server/`. Every route is a fixed `GET`: nothing runs a command, writes to the database or reads a file. Channels come from the parsed `channels.toml` and everything else from the SQLite store. Any other path gets `404` and any other method `405`.

## Settings

| Variable | Meaning |
| --- | --- |
| `SURFCULT_API_ADDR` | `ip:port` to listen on. Default `127.0.0.1:8090`, so only the Pi itself can connect. Set `0.0.0.0:8090` to allow the LAN, and set a token as well. |
| `SURFCULT_API_TOKEN` | A bearer token. When set, every request needs `Authorization: Bearer <token>` or gets `401` with `WWW-Authenticate: Bearer`. |
| `SURFCULT_API_TOKEN_FILE` | The path of a file holding the token, read when `SURFCULT_API_TOKEN` is unset. A file that cannot be read stops the server from starting rather than leaving it open. |

Without either token variable the server accepts every request.

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
