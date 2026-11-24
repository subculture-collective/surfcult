//! The library cache: a copy of the media library kept in SQLite so it can be
//! read when the source (Jellyfin) is down.
//!
//! The cache is replaced as a whole. [`Store::replace_library`] deletes
//! everything and writes the new snapshot in one transaction, so a reader sees
//! either the old library or the new one, and a failed write changes nothing.
//!
//! The cache is what the rest of surfcult reads: [`Store`] is the library's
//! [`MediaSource`], and its reads never reach Jellyfin. The Jellyfin sync step
//! (`jellyfin::sync`) is what fills it.

use std::collections::HashMap;

use rusqlite::{Transaction, params};

use super::{Store, StoreError};
use crate::library::{
    AudioStream, Chapter, Collection, MediaId, MediaItem, MediaKind, MediaSource, SourceError,
    VideoStream,
};

/// One cached item and the file path the source reported for it. The path is
/// kept next to [`MediaItem`] because the media model does not carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub item: MediaItem,
    pub path: Option<String>,
}

impl Store {
    /// In one transaction, replaces the whole library cache with `entries` and
    /// `collections`. Items and collections are read back in the order given.
    /// Every collection member must be one of `entries`, and ids must not
    /// repeat; otherwise nothing changes and an error comes back.
    pub fn replace_library(
        &mut self,
        entries: &[LibraryEntry],
        collections: &[Collection],
        synced_at_ms: i64,
    ) -> Result<(), StoreError> {
        validate(entries)?;
        let action = "replacing the library cache";
        let tx = self
            .conn
            .transaction()
            .map_err(StoreError::database(action))?;
        // Chapters, streams and collection members go with their parents.
        tx.execute_batch("DELETE FROM library_collections; DELETE FROM library_items;")
            .map_err(StoreError::database(action))?;
        for (position, entry) in entries.iter().enumerate() {
            insert_entry(&tx, position, entry, synced_at_ms)?;
        }
        for (position, collection) in collections.iter().enumerate() {
            insert_collection(&tx, position, collection)?;
        }
        tx.commit().map_err(StoreError::database(action))
    }

    /// Every cached item in sync order. Empty before the first sync.
    pub fn library_items(&self) -> Result<Vec<LibraryEntry>, StoreError> {
        let action = "reading the library cache";
        let mut statement = self
            .conn
            .prepare_cached(
                "SELECT id, title, kind, runtime_ms, path FROM library_items ORDER BY position",
            )
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map([], read_entry)
            .map_err(StoreError::database(action))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(StoreError::database(action))??);
        }
        drop(statement);

        let index: HashMap<String, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.item.id.as_str().to_string(), i))
            .collect();
        let mut chapters = self
            .conn
            .prepare_cached(
                "SELECT item_id, start_ms, name FROM library_chapters
                 ORDER BY item_id, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = chapters
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    read_chapter(row.get(1)?, row.get(2)?),
                ))
            })
            .map_err(StoreError::database(action))?;
        for row in rows {
            let (id, chapter) = row.map_err(StoreError::database(action))?;
            if let Some(&i) = index.get(&id) {
                entries[i].item.chapters.push(chapter?);
            }
        }
        drop(chapters);

        let mut streams = self
            .conn
            .prepare_cached(
                "SELECT item_id, kind, codec, width, height, bit_depth, channels, language
                 FROM library_streams ORDER BY item_id, kind, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = streams
            .query_map([], |row| Ok((row.get::<_, String>(0)?, read_stream(row)?)))
            .map_err(StoreError::database(action))?;
        for row in rows {
            let (id, stream) = row.map_err(StoreError::database(action))?;
            if let Some(&i) = index.get(&id) {
                match stream? {
                    Stream::Video(v) => entries[i].item.video.push(v),
                    Stream::Audio(a) => entries[i].item.audio.push(a),
                }
            }
        }
        Ok(entries)
    }

    /// One cached item, or `None` if the cache does not have it.
    pub fn library_item(&self, id: &MediaId) -> Result<Option<LibraryEntry>, StoreError> {
        let action = "reading a cached item";
        let mut statement = self
            .conn
            .prepare_cached(
                "SELECT id, title, kind, runtime_ms, path FROM library_items WHERE id = ?1",
            )
            .map_err(StoreError::database(action))?;
        let mut rows = statement
            .query_map([id.as_str()], read_entry)
            .map_err(StoreError::database(action))?;
        let Some(row) = rows.next() else {
            return Ok(None);
        };
        let mut entry = row.map_err(StoreError::database(action))??;
        drop(rows);
        drop(statement);

        let mut chapters = self
            .conn
            .prepare_cached(
                "SELECT start_ms, name FROM library_chapters
                 WHERE item_id = ?1 ORDER BY position",
            )
            .map_err(StoreError::database(action))?;
        let rows = chapters
            .query_map([id.as_str()], |row| {
                Ok(read_chapter(row.get(0)?, row.get(1)?))
            })
            .map_err(StoreError::database(action))?;
        for row in rows {
            entry
                .item
                .chapters
                .push(row.map_err(StoreError::database(action))??);
        }
        drop(chapters);

        let mut streams = self
            .conn
            .prepare_cached(
                "SELECT item_id, kind, codec, width, height, bit_depth, channels, language
                 FROM library_streams WHERE item_id = ?1 ORDER BY kind, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = streams
            .query_map([id.as_str()], read_stream)
            .map_err(StoreError::database(action))?;
        for row in rows {
            match row.map_err(StoreError::database(action))?? {
                Stream::Video(v) => entry.item.video.push(v),
                Stream::Audio(a) => entry.item.audio.push(a),
            }
        }
        Ok(Some(entry))
    }

    /// Every cached collection in sync order, members in the source's order.
    pub fn library_collections(&self) -> Result<Vec<Collection>, StoreError> {
        let action = "reading cached collections";
        let mut statement = self
            .conn
            .prepare_cached("SELECT position, name FROM library_collections ORDER BY position")
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StoreError::database(action))?;
        let mut collections = Vec::new();
        let mut index = HashMap::new();
        for row in rows {
            let (position, name) = row.map_err(StoreError::database(action))?;
            index.insert(position, collections.len());
            collections.push(Collection {
                name,
                members: Vec::new(),
            });
        }
        drop(statement);

        let mut members = self
            .conn
            .prepare_cached(
                "SELECT collection_position, item_id FROM library_collection_members
                 ORDER BY collection_position, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = members
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StoreError::database(action))?;
        for row in rows {
            let (position, id) = row.map_err(StoreError::database(action))?;
            if let Some(&i) = index.get(&position) {
                collections[i].members.push(MediaId::new(id));
            }
        }
        Ok(collections)
    }

    /// When the cache was last replaced, or `None` before the first sync.
    pub fn library_synced_at_ms(&self) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row("SELECT MAX(synced_at_ms) FROM library_items", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::database("reading the sync time"))
    }
}

/// The library as the cache holds it. Items and collections come back in the
/// order the last sync wrote them.
impl MediaSource for Store {
    fn list_items(&self) -> Result<Vec<MediaItem>, SourceError> {
        let entries = self.library_items().map_err(unavailable)?;
        Ok(entries.into_iter().map(|entry| entry.item).collect())
    }

    fn resolve(&self, id: &MediaId) -> Result<MediaItem, SourceError> {
        match self.library_item(id).map_err(unavailable)? {
            Some(entry) => Ok(entry.item),
            None => Err(SourceError::NotFound(id.clone())),
        }
    }

    fn list_collections(&self) -> Result<Vec<Collection>, SourceError> {
        self.library_collections().map_err(unavailable)
    }
}

fn unavailable(source: StoreError) -> SourceError {
    SourceError::Unavailable(format!("library cache: {source}"))
}

enum Stream {
    Video(VideoStream),
    Audio(AudioStream),
}

fn validate(entries: &[LibraryEntry]) -> Result<(), StoreError> {
    let invalid = |reason: String| Err(StoreError::InvalidLibrary(reason));
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let item = &entry.item;
        if !seen.insert(item.id.as_str()) {
            return invalid(format!("item {} appears twice", item.id));
        }
        if i64::try_from(item.runtime_ms).is_err() {
            return invalid(format!("runtime of {} is too large to store", item.id));
        }
        if item
            .chapters
            .iter()
            .any(|c| i64::try_from(c.start_ms).is_err())
        {
            return invalid(format!("a chapter of {} starts too late to store", item.id));
        }
    }
    Ok(())
}

fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "episode",
    }
}

fn insert_entry(
    tx: &Transaction<'_>,
    position: usize,
    entry: &LibraryEntry,
    synced_at_ms: i64,
) -> Result<(), StoreError> {
    let action = "inserting a library item";
    let item = &entry.item;
    // `validate` has checked that the runtime and chapter starts fit an i64.
    tx.prepare_cached(
        "INSERT INTO library_items (id, position, title, kind, runtime_ms, path, synced_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .and_then(|mut s| {
        s.execute(params![
            item.id.as_str(),
            position as i64,
            item.title,
            kind_name(item.kind),
            item.runtime_ms as i64,
            entry.path,
            synced_at_ms,
        ])
    })
    .map_err(StoreError::database(action))?;

    let mut chapter = tx
        .prepare_cached(
            "INSERT INTO library_chapters (item_id, position, start_ms, name)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(StoreError::database(action))?;
    for (i, c) in item.chapters.iter().enumerate() {
        chapter
            .execute(params![
                item.id.as_str(),
                i as i64,
                c.start_ms as i64,
                c.name
            ])
            .map_err(StoreError::database(action))?;
    }

    let mut stream = tx
        .prepare_cached(
            "INSERT INTO library_streams
               (item_id, kind, position, codec, width, height, bit_depth, channels, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(StoreError::database(action))?;
    for (i, v) in item.video.iter().enumerate() {
        stream
            .execute(params![
                item.id.as_str(),
                "video",
                i as i64,
                v.codec,
                v.width,
                v.height,
                v.bit_depth,
                None::<u8>,
                None::<String>,
            ])
            .map_err(StoreError::database(action))?;
    }
    for (i, a) in item.audio.iter().enumerate() {
        stream
            .execute(params![
                item.id.as_str(),
                "audio",
                i as i64,
                a.codec,
                None::<u32>,
                None::<u32>,
                None::<u8>,
                a.channels,
                a.language,
            ])
            .map_err(StoreError::database(action))?;
    }
    Ok(())
}

fn insert_collection(
    tx: &Transaction<'_>,
    position: usize,
    collection: &Collection,
) -> Result<(), StoreError> {
    let action = "inserting a collection";
    tx.execute(
        "INSERT INTO library_collections (position, name) VALUES (?1, ?2)",
        params![position as i64, collection.name],
    )
    .map_err(StoreError::database(action))?;
    let mut member = tx
        .prepare_cached(
            "INSERT INTO library_collection_members (collection_position, position, item_id)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(StoreError::database(action))?;
    for (i, id) in collection.members.iter().enumerate() {
        member
            .execute(params![position as i64, i as i64, id.as_str()])
            .map_err(|source| match source {
                rusqlite::Error::SqliteFailure(e, _)
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    StoreError::InvalidLibrary(format!(
                        "collection {:?} lists {id}, which is not a cached item, \
                         or lists it twice",
                        collection.name
                    ))
                }
                other => StoreError::database(action)(other),
            })?;
    }
    Ok(())
}

/// Reads an item row without its chapters and streams. The inner `Result`
/// carries values SQLite returned that do not fit the Rust type.
fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<LibraryEntry, StoreError>> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let runtime_ms: i64 = row.get(3)?;
    let path: Option<String> = row.get(4)?;
    Ok(build_entry(id, title, &kind, runtime_ms, path))
}

fn build_entry(
    id: String,
    title: String,
    kind: &str,
    runtime_ms: i64,
    path: Option<String>,
) -> Result<LibraryEntry, StoreError> {
    let kind = match kind {
        "movie" => MediaKind::Movie,
        "episode" => MediaKind::Episode,
        other => return Err(StoreError::Corrupt(format!("unknown item kind {other:?}"))),
    };
    let runtime_ms = u64::try_from(runtime_ms)
        .map_err(|_| StoreError::Corrupt(format!("negative runtime for {id}")))?;
    Ok(LibraryEntry {
        item: MediaItem::new(id, title, kind, runtime_ms),
        path,
    })
}

fn read_chapter(start_ms: i64, name: Option<String>) -> Result<Chapter, StoreError> {
    let start_ms = u64::try_from(start_ms)
        .map_err(|_| StoreError::Corrupt("negative chapter start".to_string()))?;
    Ok(Chapter { start_ms, name })
}

fn read_stream(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Stream, StoreError>> {
    let kind: String = row.get(1)?;
    let codec: String = row.get(2)?;
    let width: Option<u32> = row.get(3)?;
    let height: Option<u32> = row.get(4)?;
    let bit_depth: Option<u8> = row.get(5)?;
    let channels: Option<u8> = row.get(6)?;
    let language: Option<String> = row.get(7)?;
    Ok(match (kind.as_str(), width, height, channels) {
        ("video", Some(width), Some(height), _) => Ok(Stream::Video(VideoStream {
            codec,
            width,
            height,
            bit_depth,
        })),
        ("audio", _, _, Some(channels)) => Ok(Stream::Audio(AudioStream {
            codec,
            channels,
            language,
        })),
        _ => Err(StoreError::Corrupt(format!("incomplete {kind} stream"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: MediaKind) -> LibraryEntry {
        LibraryEntry {
            item: MediaItem {
                chapters: vec![
                    Chapter {
                        start_ms: 0,
                        name: None,
                    },
                    Chapter {
                        start_ms: 600_000,
                        name: Some("Act 2".into()),
                    },
                ],
                video: vec![VideoStream {
                    codec: "hevc".into(),
                    width: 1920,
                    height: 1080,
                    bit_depth: Some(10),
                }],
                audio: vec![
                    AudioStream {
                        codec: "eac3".into(),
                        channels: 6,
                        language: Some("eng".into()),
                    },
                    AudioStream {
                        codec: "aac".into(),
                        channels: 2,
                        language: None,
                    },
                ],
                ..MediaItem::new(id, format!("Title {id}"), kind, 1_320_000)
            },
            path: Some(format!("/media/{id}.mkv")),
        }
    }

    fn collection(name: &str, members: &[&str]) -> Collection {
        Collection {
            name: name.to_string(),
            members: members.iter().map(|id| MediaId::new(*id)).collect(),
        }
    }

    #[test]
    fn an_empty_store_has_no_library() {
        let store = Store::open_in_memory().unwrap();

        assert!(store.library_items().unwrap().is_empty());
        assert!(store.library_collections().unwrap().is_empty());
        assert_eq!(store.library_synced_at_ms().unwrap(), None);
        assert_eq!(store.library_item(&MediaId::new("a")).unwrap(), None);
    }

    #[test]
    fn items_round_trip_with_chapters_streams_and_path() {
        let mut store = Store::open_in_memory().unwrap();
        let entries = vec![entry("b", MediaKind::Movie), entry("a", MediaKind::Episode)];

        store.replace_library(&entries, &[], 5_000).unwrap();

        // Sync order, not id order.
        assert_eq!(store.library_items().unwrap(), entries);
        assert_eq!(
            store.library_item(&MediaId::new("a")).unwrap(),
            Some(entries[1].clone())
        );
        assert_eq!(store.library_synced_at_ms().unwrap(), Some(5_000));
    }

    #[test]
    fn a_missing_path_and_optional_fields_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let bare = LibraryEntry {
            item: MediaItem::new("x", "Bare", MediaKind::Movie, 1),
            path: None,
        };

        store
            .replace_library(std::slice::from_ref(&bare), &[], 1)
            .unwrap();

        assert_eq!(store.library_items().unwrap(), vec![bare]);
    }

    #[test]
    fn collections_keep_their_order_and_members() {
        let mut store = Store::open_in_memory().unwrap();
        let entries = vec![entry("a", MediaKind::Movie), entry("b", MediaKind::Movie)];
        let collections = vec![collection("Zed", &["b", "a"]), collection("Empty", &[])];

        store.replace_library(&entries, &collections, 1).unwrap();

        assert_eq!(store.library_collections().unwrap(), collections);
    }

    #[test]
    fn a_second_replace_removes_what_the_first_wrote() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_library(
                &[entry("a", MediaKind::Movie), entry("b", MediaKind::Movie)],
                &[collection("Old", &["a", "b"])],
                1,
            )
            .unwrap();
        let second = vec![entry("c", MediaKind::Episode)];

        store
            .replace_library(&second, &[collection("New", &["c"])], 2)
            .unwrap();

        assert_eq!(store.library_items().unwrap(), second);
        assert_eq!(store.library_item(&MediaId::new("a")).unwrap(), None);
        assert_eq!(
            store.library_collections().unwrap(),
            vec![collection("New", &["c"])]
        );
        assert_eq!(store.library_synced_at_ms().unwrap(), Some(2));
        let leftovers: i64 = store
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM library_chapters)
                      + (SELECT COUNT(*) FROM library_streams)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            leftovers,
            2 + 3,
            "only the second item's chapters and streams remain"
        );
    }

    #[test]
    fn replacing_with_nothing_empties_the_cache() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_library(&[entry("a", MediaKind::Movie)], &[], 1)
            .unwrap();

        store.replace_library(&[], &[], 2).unwrap();

        assert!(store.library_items().unwrap().is_empty());
    }

    #[test]
    fn a_collection_member_that_is_not_an_item_fails_and_changes_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        let before = vec![entry("a", MediaKind::Movie)];
        store
            .replace_library(&before, &[collection("Kept", &["a"])], 1)
            .unwrap();

        let err = store
            .replace_library(
                &[entry("b", MediaKind::Movie)],
                &[collection("Bad", &["b", "ghost"])],
                2,
            )
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidLibrary(_)), "{err}");
        assert_eq!(store.library_items().unwrap(), before);
        assert_eq!(
            store.library_collections().unwrap(),
            vec![collection("Kept", &["a"])]
        );
        assert_eq!(store.library_synced_at_ms().unwrap(), Some(1));
    }

    #[test]
    fn a_repeated_item_id_is_refused_before_anything_is_written() {
        let mut store = Store::open_in_memory().unwrap();
        let before = vec![entry("a", MediaKind::Movie)];
        store.replace_library(&before, &[], 1).unwrap();

        let err = store
            .replace_library(
                &[entry("b", MediaKind::Movie), entry("b", MediaKind::Movie)],
                &[],
                2,
            )
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidLibrary(_)));
        assert_eq!(store.library_items().unwrap(), before);
    }

    #[test]
    fn a_runtime_too_large_to_store_is_refused() {
        let mut store = Store::open_in_memory().unwrap();
        let mut huge = entry("a", MediaKind::Movie);
        huge.item.runtime_ms = u64::MAX;

        let err = store.replace_library(&[huge], &[], 1).unwrap_err();

        assert!(matches!(err, StoreError::InvalidLibrary(_)));
    }

    #[test]
    fn the_library_survives_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surfcult.db");
        let entries = vec![entry("a", MediaKind::Movie)];
        let collections = vec![collection("C", &["a"])];
        {
            let mut store = Store::open(&path).unwrap();
            store.replace_library(&entries, &collections, 9).unwrap();
        }

        let store = Store::open(&path).unwrap();

        assert_eq!(store.library_items().unwrap(), entries);
        assert_eq!(store.library_collections().unwrap(), collections);
        assert_eq!(store.library_synced_at_ms().unwrap(), Some(9));
    }

    #[test]
    fn the_library_cache_is_a_media_source() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.list_items().unwrap().is_empty());
        assert!(store.list_collections().unwrap().is_empty());
        let entries = vec![entry("b", MediaKind::Movie), entry("a", MediaKind::Episode)];
        let collections = vec![collection("Picks", &["a", "b"])];

        store
            .replace_library(&entries, &collections, 5_000)
            .unwrap();

        let items: Vec<MediaItem> = entries.iter().map(|e| e.item.clone()).collect();
        assert_eq!(store.list_items().unwrap(), items, "sync order");
        assert_eq!(store.resolve(&MediaId::new("a")).unwrap(), items[1]);
        assert_eq!(
            store.resolve(&MediaId::new("nope")).unwrap_err(),
            SourceError::NotFound(MediaId::new("nope"))
        );
        assert_eq!(store.list_collections().unwrap(), collections);
    }
}
