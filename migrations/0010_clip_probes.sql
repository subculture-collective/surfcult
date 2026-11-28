-- What ffprobe said about each clip file, so a restart does not probe an
-- unchanged file again. A row is reused only while the file at `path` (its
-- canonical path) has the same modified time and size. A file ffprobe could
-- not read keeps its reason in `error` and no duration.

CREATE TABLE clip_probes (
    path        TEXT    PRIMARY KEY,
    modified_ns INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    duration_ns INTEGER,
    error       TEXT,
    CHECK ((duration_ns IS NULL) <> (error IS NULL))
);
