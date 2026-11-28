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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Movie,
    Episode,
    MusicVideo,
    /// A song: audio with no picture of its own. Only a channel with a
    /// `picture` plays songs ([`Plays::Songs`]).
    Song,
}

impl MediaKind {
    /// Whether a channel that plays `plays` may schedule an item of this kind.
    pub fn played_by(self, plays: Plays) -> bool {
        (self == Self::Song) == (plays == Plays::Songs)
    }
}

/// What a channel plays: videos (movies, episodes and music videos) or
/// songs. A channel with a `picture` plays songs and nothing else; every
/// other channel plays videos and never a song, whatever its sources match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Plays {
    #[default]
    Videos,
    Songs,
}

/// A chapter mark, used to pick commercial-break cut points.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chapter {
    pub start_ms: u64,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub bit_depth: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AudioStream {
    pub codec: String,
    pub channels: u8,
    pub language: Option<String>,
}

/// Where an episode sits in its series.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeriesEpisode {
    /// The series' id in the source (a Jellyfin series id, for Jellyfin).
    pub series_id: MediaId,
    pub series_name: String,
    /// The season number. Jellyfin numbers specials as season 0.
    pub season: Option<u32>,
    /// The episode number within the season.
    pub number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    /// The music video's or song's artists, in the source's order. Empty for
    /// other kinds.
    pub artists: Vec<String>,
    /// The music video's or song's album, when the source names one.
    pub album: Option<String>,
    /// The song's album id in the source, whose primary image is the cover.
    /// `None` for other kinds and for a song the source puts on no album.
    pub album_id: Option<MediaId>,
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
            artists: Vec::new(),
            album: None,
            album_id: None,
        }
    }

    /// The title the banner and the guide show: `Series: Title` for an
    /// episode in a named series, `Artist – Title` for an item with artists
    /// (several are joined with commas), and the plain title otherwise.
    pub fn display_title(&self) -> String {
        match &self.series {
            Some(series) if !series.series_name.is_empty() => {
                return format!("{}: {}", series.series_name, self.title);
            }
            _ => {}
        }
        if self.artists.is_empty() {
            self.title.clone()
        } else {
            format!("{} – {}", self.artists.join(", "), self.title)
        }
    }
}

/// A named group of items, such as a Jellyfin collection. Members are listed
/// in the source's order. A member is an item id or a series id; a series
/// stands for all of its episodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Collection {
    pub name: String,
    pub members: Vec<MediaId>,
}
