-- Schedule events, per-channel cursors and playback history.
-- Channel numbers come from channels.toml; they are not rows in this database.
-- All times are Unix epoch milliseconds.

CREATE TABLE events (
    channel           INTEGER NOT NULL,
    start_ms          INTEGER NOT NULL,
    end_ms            INTEGER NOT NULL,
    media_id          TEXT    NOT NULL,
    media_offset_ms   INTEGER NOT NULL CHECK (media_offset_ms >= 0),
    media_duration_ms INTEGER NOT NULL CHECK (media_duration_ms >= 0),
    kind              TEXT    NOT NULL CHECK (kind IN ('program', 'break', 'filler', 'ident')),
    -- Events of one channel do not overlap, so a start time identifies one.
    -- This key is also the index for reading a channel's events by time.
    PRIMARY KEY (channel, start_ms),
    CHECK (end_ms > start_ms)
) WITHOUT ROWID;

CREATE TABLE cursors (
    channel       INTEGER PRIMARY KEY,
    -- The media id of the last item a sequential channel scheduled.
    last_media_id TEXT NOT NULL
);

CREATE TABLE playback_history (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    channel        INTEGER NOT NULL,
    media_id       TEXT    NOT NULL,
    started_at_ms  INTEGER NOT NULL,
    -- A short word such as "played" or "failed"; the tuner chooses it.
    outcome        TEXT    NOT NULL
);

CREATE INDEX playback_history_by_channel ON playback_history (channel, started_at_ms);
