-- Songs: a fourth item kind, a song's album id, an index for finding the
-- items with one label, and the time of the last sync.
--
-- A song's album id names the album whose primary image is its cover. The
-- index lets a genre source read only its own items from a cache that holds
-- tens of thousands of songs.

INSERT INTO library_item_kinds (name) VALUES ('song');

-- Set for songs only, and not always then.
ALTER TABLE library_items ADD COLUMN album_id TEXT;

CREATE INDEX library_item_labels_by_name ON library_item_labels (kind, name, item_id);

-- When the last sync finished, whether it replaced the cache or found the
-- library unchanged and left it. At most one row. A sync of a library that
-- holds tens of thousands of songs costs seconds to write on a Raspberry Pi,
-- so an unchanged library is not written again; this row still moves.
CREATE TABLE library_sync (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    synced_at_ms INTEGER NOT NULL
);

INSERT INTO library_sync (id, synced_at_ms)
    SELECT 1, MAX(synced_at_ms) FROM library_items HAVING COUNT(*) > 0;
