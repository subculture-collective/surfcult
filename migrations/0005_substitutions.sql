-- Scheduled programs that a regeneration replaced because their item left
-- the library. Times are Unix epoch milliseconds.

CREATE TABLE substitutions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    channel        INTEGER NOT NULL,
    -- The item that left the library and when its program was to start.
    media_id       TEXT    NOT NULL,
    scheduled_ms   INTEGER NOT NULL,
    -- What the new schedule has at that time, or NULL when nothing is on.
    replacement_id TEXT,
    recorded_at_ms INTEGER NOT NULL
);

CREATE INDEX substitutions_by_channel ON substitutions (channel, scheduled_ms);
