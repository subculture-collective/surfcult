-- Music videos: a third item kind, an item's artists and its album.
--
-- The item kinds move from a CHECK constraint into a table, so a later kind is
-- one INSERT rather than another rebuild. Artists are labels, like tags and
-- genres, in Jellyfin's order. The album is a column, set for music videos.
--
-- SQLite cannot change a CHECK constraint in place, so `library_items` is
-- rebuilt. Dropping it would delete its chapters, streams and labels through
-- their foreign keys, so those tables are rebuilt too and every row is copied.
-- Renaming `library_items_new` rewrites the children's references to it.

CREATE TABLE library_item_kinds (
    name TEXT PRIMARY KEY
) WITHOUT ROWID;

INSERT INTO library_item_kinds (name) VALUES ('movie'), ('episode'), ('music_video');

CREATE TABLE library_items_new (
    -- The Jellyfin item id.
    id           TEXT    PRIMARY KEY,
    position     INTEGER NOT NULL,
    title        TEXT    NOT NULL,
    kind         TEXT    NOT NULL REFERENCES library_item_kinds (name),
    runtime_ms   INTEGER NOT NULL CHECK (runtime_ms >= 0),
    -- The file path as Jellyfin sees it; Jellyfin does not always report one.
    path         TEXT,
    synced_at_ms INTEGER NOT NULL,
    -- Set for episodes only. `series_id` is the Jellyfin series id.
    series_id    TEXT,
    series_name  TEXT,
    season       INTEGER CHECK (season >= 0),
    episode      INTEGER CHECK (episode >= 0),
    -- Set for music videos only, and not always then.
    album        TEXT
);

INSERT INTO library_items_new
       (id, position, title, kind, runtime_ms, path, synced_at_ms,
        series_id, series_name, season, episode)
    SELECT id, position, title, kind, runtime_ms, path, synced_at_ms,
           series_id, series_name, season, episode
    FROM library_items;

CREATE TABLE library_chapters_new (
    item_id  TEXT    NOT NULL REFERENCES library_items_new (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    name     TEXT,
    PRIMARY KEY (item_id, position)
) WITHOUT ROWID;

INSERT INTO library_chapters_new SELECT item_id, position, start_ms, name FROM library_chapters;

-- Video and audio streams share a table. `position` counts within one kind.
CREATE TABLE library_streams_new (
    item_id   TEXT    NOT NULL REFERENCES library_items_new (id) ON DELETE CASCADE,
    kind      TEXT    NOT NULL CHECK (kind IN ('video', 'audio')),
    position  INTEGER NOT NULL,
    codec     TEXT    NOT NULL,
    -- Video only.
    width     INTEGER,
    height    INTEGER,
    bit_depth INTEGER,
    -- Audio only.
    channels  INTEGER,
    language  TEXT,
    PRIMARY KEY (item_id, kind, position),
    CHECK (kind <> 'video' OR (width IS NOT NULL AND height IS NOT NULL)),
    CHECK (kind <> 'audio' OR channels IS NOT NULL)
) WITHOUT ROWID;

INSERT INTO library_streams_new
    SELECT item_id, kind, position, codec, width, height, bit_depth, channels, language
    FROM library_streams;

-- Library names, tags, genres and artists share a table. `position` counts
-- within one kind and keeps Jellyfin's order.
CREATE TABLE library_item_labels_new (
    item_id  TEXT    NOT NULL REFERENCES library_items_new (id) ON DELETE CASCADE,
    kind     TEXT    NOT NULL CHECK (kind IN ('library', 'tag', 'genre', 'artist')),
    position INTEGER NOT NULL,
    name     TEXT    NOT NULL,
    PRIMARY KEY (item_id, kind, position)
) WITHOUT ROWID;

INSERT INTO library_item_labels_new SELECT item_id, kind, position, name FROM library_item_labels;

DROP TABLE library_chapters;
DROP TABLE library_streams;
DROP TABLE library_item_labels;
DROP TABLE library_items;

ALTER TABLE library_items_new RENAME TO library_items;
ALTER TABLE library_chapters_new RENAME TO library_chapters;
ALTER TABLE library_streams_new RENAME TO library_streams;
ALTER TABLE library_item_labels_new RENAME TO library_item_labels;

CREATE INDEX library_items_by_position ON library_items (position);
