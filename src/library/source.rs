use std::error::Error;
use std::fmt;

use super::model::{Collection, MediaId, MediaItem, Plays};

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

    /// The items `query` matches, in [`MediaSource::list_items`] order. The
    /// library cache answers this in SQL, so a video channel never reads the
    /// songs and a genre never reads the whole library.
    fn query_items(&self, query: &ItemQuery<'_>) -> Result<Vec<MediaItem>, SourceError> {
        let mut items = self.list_items()?;
        items.retain(|item| query.matches(item));
        Ok(items)
    }

    /// The episodes, in [`MediaSource::list_items`] order, of the first
    /// series in that order named `name` among the items a channel playing
    /// `plays` may schedule; empty when there is none. The library cache
    /// answers this in SQL from an index, so a series source does not read
    /// the rest of the library.
    fn series_episodes(&self, plays: Plays, name: &str) -> Result<Vec<MediaItem>, SourceError> {
        let items = self.query_items(&ItemQuery { plays, label: None })?;
        let Some(series) = items.iter().find_map(|item| {
            item.series
                .as_ref()
                .filter(|series| series.series_name == name)
                .map(|series| series.series_id.clone())
        }) else {
            return Ok(Vec::new());
        };
        Ok(items
            .into_iter()
            .filter(|item| item.series.as_ref().map(|s| &s.series_id) == Some(&series))
            .collect())
    }
}

/// Which items a program pool asks for: those a channel playing `plays` may
/// schedule, and, with a `label`, only those in that library or with that
/// tag or genre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemQuery<'a> {
    pub plays: Plays,
    pub label: Option<Label<'a>>,
}

/// A library, tag or genre name. Names match exactly, including case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label<'a> {
    Library(&'a str),
    Tag(&'a str),
    Genre(&'a str),
}

impl ItemQuery<'_> {
    pub fn matches(&self, item: &MediaItem) -> bool {
        let has = |names: &[String], name: &str| names.iter().any(|n| n == name);
        item.kind.played_by(self.plays)
            && match self.label {
                None => true,
                Some(Label::Library(name)) => has(&item.libraries, name),
                Some(Label::Tag(name)) => has(&item.tags, name),
                Some(Label::Genre(name)) => has(&item.genres, name),
            }
    }
}
