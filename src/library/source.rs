use std::error::Error;
use std::fmt;

use super::model::{Collection, MediaId, MediaItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The source has no item with this id.
    NotFound(MediaId),
    /// The source could not be reached or answered with an error.
    Unavailable(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "media item {id} not found"),
            Self::Unavailable(reason) => write!(f, "media source unavailable: {reason}"),
        }
    }
}

impl Error for SourceError {}

/// Where media comes from. surfcult reads media through this trait and never
/// touches files or transcodes itself.
pub trait MediaSource {
    /// Every item the source offers, in a stable order.
    fn list_items(&self) -> Result<Vec<MediaItem>, SourceError>;

    /// One item by id.
    fn resolve(&self, id: &MediaId) -> Result<MediaItem, SourceError>;

    /// Every collection the source offers, in a stable order.
    fn list_collections(&self) -> Result<Vec<Collection>, SourceError>;
}
