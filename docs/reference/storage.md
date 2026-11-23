# Storage

`channels.toml` is the source of truth for channels. SQLite holds what the program derives from it and what happens at run time. The code is in `src/store/` and the schema is in `migrations/`.

The `events` table holds each channel's generated schedule, one row per `timeline::Event`. Rows are keyed by channel number and start time, which is also the index for reading a time range; events of one channel must not overlap. `cursors` holds one row per sequential channel: the media id of the last item the generator scheduled, so the next run continues after that item even if the library has changed. `playback_history` gets one row each time the tuner starts something, with a short outcome word such as `played` or `failed`. All times are Unix epoch milliseconds. `Store` opens the file in WAL mode with a 5 second busy timeout and foreign keys on, and it refuses to write an event that does not end after it starts.

Migrations are numbered SQL files (`migrations/0001_schedule.sql`) compiled into the binary and listed in `src/store/migrate.rs`. On open, each migration the database has not seen runs once, in order, in its own transaction, and is recorded in `schema_migrations`. To change the schema, add the next numbered file and list it; never edit one that has shipped. A database written by a newer build is refused rather than opened.
