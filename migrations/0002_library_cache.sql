-- A local copy of the Jellyfin library, so the library is readable when
-- Jellyfin is not. A sync replaces all of these tables in one transaction.
-- `position` columns keep the order Jellyfin returned things in. Times are Unix
-- epoch milliseconds.

CREATE TABLE library_items (
    -- The Jellyfin item id.
    id           TEXT    PRIMARY KEY,
    position     INTEGER NOT NULL,
    title        TEXT    NOT NULL,
    kind         TEXT    NOT NULL CHECK (kind IN ('movie', 'episode')),
    runtime_ms   INTEGER NOT NULL CHECK (runtime_ms >= 0),
    -- The file path as Jellyfin sees it; Jellyfin does not always report one.
    path         TEXT,
    synced_at_ms INTEGER NOT NULL
);

CREATE INDEX library_items_by_position ON library_items (position);

CREATE TABLE library_chapters (
    item_id  TEXT    NOT NULL REFERENCES library_items (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    name     TEXT,
    PRIMARY KEY (item_id, position)
) WITHOUT ROWID;

-- Video and audio streams share a table. `position` counts within one kind.
CREATE TABLE library_streams (
    item_id   TEXT    NOT NULL REFERENCES library_items (id) ON DELETE CASCADE,
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

-- Collections are listed by name, so they have no id of their own here.
CREATE TABLE library_collections (
    position INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL
);

CREATE TABLE library_collection_members (
    collection_position INTEGER NOT NULL
        REFERENCES library_collections (position) ON DELETE CASCADE,
    position            INTEGER NOT NULL,
    item_id             TEXT    NOT NULL REFERENCES library_items (id) ON DELETE CASCADE,
    PRIMARY KEY (collection_position, position)
) WITHOUT ROWID;
