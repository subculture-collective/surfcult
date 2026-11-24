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

use std::collections::{HashMap, HashSet};

use rusqlite::{Transaction, params};

use super::{Store, StoreError};
use crate::library::{
    AudioStream, Chapter, Collection, MediaId, MediaItem, MediaKind, MediaSource, SeriesEpisode,
    SourceError, VideoStream,
};

/// One cached item and the file path the source reported for it. The path is
/// kept next to [`MediaItem`] because the media model does not carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub item: MediaItem,
    pub path: Option<String>,
}

/// The columns of `library_items` that [`read_entry`] reads, in its order.
const ITEM_COLUMNS: &str =
    "id, title, kind, runtime_ms, path, series_id, series_name, season, episode";

impl Store {
    /// In one transaction, replaces the whole library cache with `entries` and
    /// `collections`. Items and collections are read back in the order given.
    /// Every collection member must be the id of one of `entries` or the series
    /// id of one of them, and item ids must not repeat; otherwise nothing
    /// changes and an error comes back.
    pub fn replace_library(
        &mut self,
        entries: &[LibraryEntry],
        collections: &[Collection],
        synced_at_ms: i64,
    ) -> Result<(), StoreError> {
        validate(entries, collections)?;
        let action = "replacing the library cache";
        let tx = self
            .conn
            .transaction()
            .map_err(StoreError::database(action))?;
        // Chapters, streams, labels and collection members go with their parents.
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
            .prepare_cached(&format!(
                "SELECT {ITEM_COLUMNS} FROM library_items ORDER BY position"
            ))
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
        drop(streams);

        let mut labels = self
            .conn
            .prepare_cached(
                "SELECT item_id, kind, name FROM library_item_labels
                 ORDER BY item_id, kind, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = labels
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(StoreError::database(action))?;
        for row in rows {
            let (id, kind, name) = row.map_err(StoreError::database(action))?;
            if let Some(&i) = index.get(&id) {
                push_label(&mut entries[i].item, &kind, name)?;
            }
        }
        Ok(entries)
    }

    /// One cached item, or `None` if the cache does not have it.
    pub fn library_item(&self, id: &MediaId) -> Result<Option<LibraryEntry>, StoreError> {
        let action = "reading a cached item";
        let mut statement = self
            .conn
            .prepare_cached(&format!(
                "SELECT {ITEM_COLUMNS} FROM library_items WHERE id = ?1"
            ))
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
        drop(streams);

        let mut labels = self
            .conn
            .prepare_cached(
                "SELECT kind, name FROM library_item_labels
                 WHERE item_id = ?1 ORDER BY kind, position",
            )
            .map_err(StoreError::database(action))?;
        let rows = labels
            .query_map([id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StoreError::database(action))?;
        for row in rows {
            let (kind, name) = row.map_err(StoreError::database(action))?;
            push_label(&mut entry.item, &kind, name)?;
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
                "SELECT collection_position, member_id FROM library_collection_members
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

fn validate(entries: &[LibraryEntry], collections: &[Collection]) -> Result<(), StoreError> {
    let invalid = |reason: String| Err(StoreError::InvalidLibrary(reason));
    let mut seen = HashSet::new();
    let mut series = HashSet::new();
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
        if let Some(episode) = &item.series {
            series.insert(episode.series_id.as_str());
        }
    }
    for collection in collections {
        for id in &collection.members {
            if !seen.contains(id.as_str()) && !series.contains(id.as_str()) {
                return invalid(format!(
                    "collection {:?} lists {id}, which is neither a cached item \
                     nor the series of one",
                    collection.name
                ));
            }
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
    let series = item.series.as_ref();
    // `validate` has checked that the runtime and chapter starts fit an i64.
    tx.prepare_cached(
        "INSERT INTO library_items (id, position, title, kind, runtime_ms, path, synced_at_ms,
                                    series_id, series_name, season, episode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            series.map(|s| s.series_id.as_str()),
            series.map(|s| s.series_name.as_str()),
            series.and_then(|s| s.season),
            series.and_then(|s| s.number),
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

    let mut label = tx
        .prepare_cached(
            "INSERT INTO library_item_labels (item_id, kind, position, name)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(StoreError::database(action))?;
    for (kind, names) in [
        ("library", &item.libraries),
        ("tag", &item.tags),
        ("genre", &item.genres),
    ] {
        for (i, name) in names.iter().enumerate() {
            label
                .execute(params![item.id.as_str(), kind, i as i64, name])
                .map_err(StoreError::database(action))?;
        }
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
            "INSERT INTO library_collection_members (collection_position, position, member_id)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(StoreError::database(action))?;
    for (i, id) in collection.members.iter().enumerate() {
        member
            .execute(params![position as i64, i as i64, id.as_str()])
            .map_err(StoreError::database(action))?;
    }
    Ok(())
}

/// Reads an item row ([`ITEM_COLUMNS`]) without its chapters, streams and
/// labels. The inner `Result` carries values SQLite returned that do not fit
/// the Rust type.
fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<LibraryEntry, StoreError>> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let runtime_ms: i64 = row.get(3)?;
    let path: Option<String> = row.get(4)?;
    let series_id: Option<String> = row.get(5)?;
    let series_name: Option<String> = row.get(6)?;
    let season: Option<i64> = row.get(7)?;
    let episode: Option<i64> = row.get(8)?;
    Ok(
        build_entry(id, title, &kind, runtime_ms, path).and_then(|mut entry| {
            entry.item.series = build_series(series_id, series_name, season, episode)?;
            Ok(entry)
        }),
    )
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

/// An episode's series, or `None` when the row has no series id. A missing
/// name is read as the id.
fn build_series(
    series_id: Option<String>,
    series_name: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
) -> Result<Option<SeriesEpisode>, StoreError> {
    let Some(series_id) = series_id else {
        return Ok(None);
    };
    let number = |value: Option<i64>| {
        value
            .map(u32::try_from)
            .transpose()
            .map_err(|_| StoreError::Corrupt(format!("bad episode number in {series_id}")))
    };
    Ok(Some(SeriesEpisode {
        season: number(season)?,
        number: number(episode)?,
        series_name: series_name.unwrap_or_else(|| series_id.clone()),
        series_id: MediaId::new(series_id),
    }))
}

fn push_label(item: &mut MediaItem, kind: &str, name: String) -> Result<(), StoreError> {
    match kind {
        "library" => item.libraries.push(name),
        "tag" => item.tags.push(name),
        "genre" => item.genres.push(name),
        other => return Err(StoreError::Corrupt(format!("unknown label kind {other:?}"))),
    }
    Ok(())
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

    /// An episode of series `series` with libraries, tags and genres.
    fn labelled_episode(id: &str, series: &str) -> LibraryEntry {
        let mut entry = entry(id, MediaKind::Episode);
        entry.item.libraries = vec!["TV".into(), "Kids".into()];
        entry.item.tags = vec!["Saturday".into(), "Retro".into()];
        entry.item.genres = vec!["Animation".into()];
        entry.item.series = Some(SeriesEpisode {
            series_id: MediaId::new(series),
            series_name: format!("Series {series}"),
            season: Some(0),
            number: Some(12),
        });
        entry
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
    fn libraries_tags_genres_and_series_round_trip_in_order() {
        let mut store = Store::open_in_memory().unwrap();
        let mut unnamed = labelled_episode("b", "s2");
        unnamed.item.series.as_mut().unwrap().season = None;
        unnamed.item.series.as_mut().unwrap().number = None;
        let entries = vec![
            labelled_episode("a", "s1"),
            unnamed,
            entry("m", MediaKind::Movie),
        ];

        store.replace_library(&entries, &[], 1).unwrap();

        assert_eq!(store.library_items().unwrap(), entries);
        assert_eq!(
            store.library_item(&MediaId::new("a")).unwrap(),
            Some(entries[0].clone())
        );
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
    fn a_collection_may_list_a_series_of_a_cached_episode() {
        let mut store = Store::open_in_memory().unwrap();
        let entries = vec![
            labelled_episode("e", "cheers"),
            entry("m", MediaKind::Movie),
        ];
        let collections = vec![collection("Mixed", &["cheers", "m"])];

        store.replace_library(&entries, &collections, 1).unwrap();

        assert_eq!(store.library_collections().unwrap(), collections);
    }

    #[test]
    fn a_second_replace_removes_what_the_first_wrote() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .replace_library(
                &[labelled_episode("a", "s"), entry("b", MediaKind::Movie)],
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
                      + (SELECT COUNT(*) FROM library_streams)
                      + (SELECT COUNT(*) FROM library_item_labels)
                      + (SELECT COUNT(*) FROM library_collection_members)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            leftovers,
            2 + 3 + 1,
            "only the second item's chapters and streams and the new member remain"
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
    fn a_collection_member_that_is_not_an_item_or_series_fails_and_changes_nothing() {
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
        assert!(err.to_string().contains("ghost"), "{err}");
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
        let entries = vec![entry("a", MediaKind::Movie), labelled_episode("e", "s")];
        let collections = vec![collection("C", &["a", "s"])];
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
