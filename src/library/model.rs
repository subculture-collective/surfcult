use std::fmt;

/// Identifies an item within its source (a Jellyfin item id, for Jellyfin).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(String);

impl MediaId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Movie,
    Episode,
}

/// A chapter mark, used to pick commercial-break cut points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub start_ms: u64,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub bit_depth: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    pub codec: String,
    pub channels: u8,
    pub language: Option<String>,
}

/// Where an episode sits in its series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesEpisode {
    /// The series' id in the source (a Jellyfin series id, for Jellyfin).
    pub series_id: MediaId,
    pub series_name: String,
    /// The season number. Jellyfin numbers specials as season 0.
    pub season: Option<u32>,
    /// The episode number within the season.
    pub number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: MediaId,
    pub title: String,
    pub kind: MediaKind,
    pub runtime_ms: u64,
    pub chapters: Vec<Chapter>,
    pub video: Vec<VideoStream>,
    pub audio: Vec<AudioStream>,
    /// The names of the libraries the item is in, in the source's order.
    pub libraries: Vec<String>,
    /// The item's tags. An episode also carries its series' tags.
    pub tags: Vec<String>,
    /// The item's genres. An episode also carries its series' genres.
    pub genres: Vec<String>,
    /// The episode's series, or `None` for a movie or for an episode the
    /// source does not place in a series.
    pub series: Option<SeriesEpisode>,
}

impl MediaItem {
    /// An item with no chapters, streams, libraries, tags, genres or series;
    /// fill those in with struct update syntax or by assigning the fields.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: MediaKind,
        runtime_ms: u64,
    ) -> Self {
        Self {
            id: MediaId::new(id),
            title: title.into(),
            kind,
            runtime_ms,
            chapters: Vec::new(),
            video: Vec::new(),
            audio: Vec::new(),
            libraries: Vec::new(),
            tags: Vec::new(),
            genres: Vec::new(),
            series: None,
        }
    }
}

/// A named group of items, such as a Jellyfin collection. Members are listed
/// in the source's order. A member is an item id or a series id; a series
/// stands for all of its episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub name: String,
    pub members: Vec<MediaId>,
}
