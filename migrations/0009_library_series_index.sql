-- Indexes for finding one series in a library cache that holds tens of
-- thousands of songs: by name, to find the first series with that name,
-- and by id, to read only its episodes. Also the fingerprint of the
-- library the last sync wrote, so the first sync after a restart does not
-- rewrite an unchanged cache.

CREATE INDEX library_items_by_series_name ON library_items (series_name, position);
CREATE INDEX library_items_by_series ON library_items (series_id);

-- NULL until a sync records one, and set back to NULL whenever the cache is
-- replaced without one.
ALTER TABLE library_sync ADD COLUMN fingerprint INTEGER;
