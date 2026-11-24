-- What a channel source needs to find items in the library cache: the
-- libraries, tags and genres of each item, the series an episode belongs to
-- and its place there, and collections that list a series. Rows cached before
-- this migration have none of it until the next sync replaces them.

-- Set for episodes only. `series_id` is the Jellyfin series id.
ALTER TABLE library_items ADD COLUMN series_id   TEXT;
ALTER TABLE library_items ADD COLUMN series_name TEXT;
ALTER TABLE library_items ADD COLUMN season      INTEGER CHECK (season >= 0);
ALTER TABLE library_items ADD COLUMN episode     INTEGER CHECK (episode >= 0);

-- Library names, tags and genres share a table. `position` counts within one
-- kind and keeps Jellyfin's order.
CREATE TABLE library_item_labels (
    item_id  TEXT    NOT NULL REFERENCES library_items (id) ON DELETE CASCADE,
    kind     TEXT    NOT NULL CHECK (kind IN ('library', 'tag', 'genre')),
    position INTEGER NOT NULL,
    name     TEXT    NOT NULL,
    PRIMARY KEY (item_id, kind, position)
) WITHOUT ROWID;

-- A collection member is now an item id or a series id, so it can no longer
-- reference library_items. `Store::replace_library` checks each member
-- instead. The table is rebuilt because SQLite cannot drop a foreign key.
CREATE TABLE library_collection_members_new (
    collection_position INTEGER NOT NULL
        REFERENCES library_collections (position) ON DELETE CASCADE,
    position            INTEGER NOT NULL,
    member_id           TEXT    NOT NULL,
    PRIMARY KEY (collection_position, position)
) WITHOUT ROWID;

INSERT INTO library_collection_members_new (collection_position, position, member_id)
    SELECT collection_position, position, item_id FROM library_collection_members;

DROP TABLE library_collection_members;

ALTER TABLE library_collection_members_new RENAME TO library_collection_members;
