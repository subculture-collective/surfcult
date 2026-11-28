//! What Jellyfin's JSON looks like, and how it becomes media items.
//!
//! Only the fields surfcult needs are named. Jellyfin sends many more and adds
//! new ones between versions; serde ignores what is not named here, and every
//! field except an item's `Id` may be absent or `null`.

use serde::Deserialize;
use serde_json::Value;

use crate::library::{
    AudioStream, Chapter, MediaId, MediaItem, MediaKind, SeriesEpisode, VideoStream,
};
use crate::store::LibraryEntry;

/// Jellyfin counts time in ticks of 100 nanoseconds.
pub(super) const TICKS_PER_MS: i64 = 10_000;

/// One page of `/Items`. The items stay as raw JSON so that one item with an
/// unexpected shape can be skipped without failing the whole page.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Page {
    pub items: Vec<Value>,
    pub total_record_count: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawItem {
    id: String,
    name: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    run_time_ticks: Option<i64>,
    path: Option<String>,
    media_sources: Option<Vec<RawSource>>,
    media_streams: Option<Vec<RawStream>>,
    chapters: Option<Vec<RawChapter>>,
    tags: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    series_id: Option<String>,
    series_name: Option<String>,
    /// The season number, for an episode.
    parent_index_number: Option<i64>,
    /// The episode number within the season.
    index_number: Option<i64>,
    /// A music video's artists.
    artists: Option<Vec<String>>,
    album: Option<String>,
}

/// A series as listed by `/Items`, with the tags and genres its episodes
/// inherit.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct RawSeries {
    pub id: String,
    pub tags: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawSource {
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawStream {
    #[serde(rename = "Type")]
    kind: Option<String>,
    codec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawChapter {
    start_position_ticks: Option<i64>,
    name: Option<String>,
}

/// A collection (`BoxSet`) as listed by `/Items`, or a library as listed by
/// `/Library/MediaFolders`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct RawCollection {
    pub id: String,
    pub name: Option<String>,
}

/// An item that only has an id, as when listing a collection's or a
/// library's children.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct RawChild {
    pub id: String,
}

/// What became of one item in a page.
pub(super) enum Parsed {
    Playable(Box<LibraryEntry>),
    /// Not playable: no video stream (Jellyfin lists missing episodes), no
    /// runtime, or not a movie, episode or music video.
    Skipped,
    /// The JSON did not have the shape of an item.
    Invalid,
}

pub(super) fn parse_item(value: Value) -> Parsed {
    let Ok(raw) = serde_json::from_value::<RawItem>(value) else {
        return Parsed::Invalid;
    };
    let kind = match raw.kind.as_deref() {
        Some("Movie") => MediaKind::Movie,
        Some("Episode") => MediaKind::Episode,
        Some("MusicVideo") => MediaKind::MusicVideo,
        _ => return Parsed::Skipped,
    };
    let Some(runtime_ms) = raw
        .run_time_ticks
        .filter(|ticks| *ticks > 0)
        .map(|ticks| (ticks / TICKS_PER_MS) as u64)
    else {
        return Parsed::Skipped;
    };

    let mut video = Vec::new();
    let mut audio = Vec::new();
    for stream in raw.media_streams.unwrap_or_default() {
        match stream.kind.as_deref() {
            Some("Video") => video.push(VideoStream {
                codec: stream.codec.unwrap_or_else(|| "unknown".to_string()),
                width: stream.width.unwrap_or(0),
                height: stream.height.unwrap_or(0),
                bit_depth: stream.bit_depth,
            }),
            Some("Audio") => audio.push(AudioStream {
                codec: stream.codec.unwrap_or_else(|| "unknown".to_string()),
                channels: stream.channels.unwrap_or(0),
                language: non_empty(stream.language),
            }),
            _ => {}
        }
    }
    if video.is_empty() {
        return Parsed::Skipped;
    }

    let chapters = raw
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| Chapter {
            start_ms: (chapter.start_position_ticks.unwrap_or(0).max(0) / TICKS_PER_MS) as u64,
            name: non_empty(chapter.name),
        })
        .collect();

    let path = non_empty(raw.path).or_else(|| {
        raw.media_sources
            .unwrap_or_default()
            .into_iter()
            .find_map(|source| non_empty(source.path))
    });
    let title = non_empty(raw.name).unwrap_or_else(|| raw.id.clone());
    let number = |value: Option<i64>| value.and_then(|n| u32::try_from(n).ok());
    let series = match (kind, non_empty(raw.series_id)) {
        (MediaKind::Episode, Some(series_id)) => Some(SeriesEpisode {
            series_name: non_empty(raw.series_name).unwrap_or_else(|| series_id.clone()),
            series_id: MediaId::new(series_id),
            season: number(raw.parent_index_number),
            number: number(raw.index_number),
        }),
        _ => None,
    };
    let (artists, album) = match kind {
        MediaKind::MusicVideo => (names(raw.artists), non_empty(raw.album)),
        MediaKind::Movie | MediaKind::Episode => (Vec::new(), None),
    };

    Parsed::Playable(Box::new(LibraryEntry {
        item: MediaItem {
            chapters,
            video,
            audio,
            tags: names(raw.tags),
            genres: names(raw.genres),
            series,
            artists,
            album,
            ..MediaItem::new(raw.id, title, kind, runtime_ms)
        },
        path,
    }))
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
}

/// A list of tag, genre or artist names without empty ones.
pub(super) fn names(list: Option<Vec<String>>) -> Vec<String> {
    let mut list = list.unwrap_or_default();
    list.retain(|name| !name.is_empty());
    list
}
