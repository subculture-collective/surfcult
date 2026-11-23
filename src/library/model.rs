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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: MediaId,
    pub title: String,
    pub kind: MediaKind,
    pub runtime_ms: u64,
    pub chapters: Vec<Chapter>,
    pub video: Vec<VideoStream>,
    pub audio: Vec<AudioStream>,
}

impl MediaItem {
    /// An item with no chapters or streams; fill those in with struct update
    /// syntax or by assigning the fields.
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
        }
    }
}

/// A named group of items, such as a Jellyfin collection. Members are listed
/// in the source's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub name: String,
    pub members: Vec<MediaId>,
}
