use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use super::model::MediaItem;
use super::source::{MediaSource, SourceError};
use crate::config::Source;

/// Why a channel's source could not be turned into a program pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// The source kind cannot be resolved yet. Libraries, tags, genres and
    /// series need issue #30, which turns them into episodes.
    Unsupported(Source),
    /// No collection in the media source has this exact name.
    CollectionNotFound(String),
    /// The media source failed.
    Source(SourceError),
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(source) => {
                let (key, name) = match source {
                    Source::JellyfinLibrary(name) => ("jellyfin_library", name),
                    Source::JellyfinCollection(name) => ("jellyfin_collection", name),
                    Source::JellyfinTag(name) => ("jellyfin_tag", name),
                    Source::JellyfinGenre(name) => ("jellyfin_genre", name),
                    Source::JellyfinSeries(name) => ("jellyfin_series", name),
                };
                write!(f, "{key} {name:?} cannot be resolved yet (#30)")
            }
            Self::CollectionNotFound(name) => write!(f, "no collection named {name:?}"),
            Self::Source(error) => write!(f, "media source: {error}"),
        }
    }
}

impl Error for PoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SourceError> for PoolError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

/// Turns a channel's configured source into its program pool: the items to
/// schedule, in the order [`schedule::extend`](crate::schedule::extend)
/// expects.
///
/// Only [`Source::JellyfinCollection`] resolves. It takes the first collection
/// whose name equals `name` exactly and returns its members in collection
/// order. A member the media source does not list is skipped, and a member that
/// appears twice is kept once, at its first place. An empty collection gives an
/// empty pool, which `extend` reports as `Note::NoPlayableItems`.
///
/// Every other source returns [`PoolError::Unsupported`] rather than every
/// item, so a channel never silently plays the whole library.
pub fn resolve_pool(
    source: &Source,
    media: &impl MediaSource,
) -> Result<Vec<MediaItem>, PoolError> {
    let Source::JellyfinCollection(name) = source else {
        return Err(PoolError::Unsupported(source.clone()));
    };
    let collection = media
        .list_collections()?
        .into_iter()
        .find(|collection| &collection.name == name)
        .ok_or_else(|| PoolError::CollectionNotFound(name.clone()))?;
    let mut items: HashMap<_, _> = media
        .list_items()?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();
    // Removing an item as it is taken skips both uncached and repeated members.
    Ok(collection
        .members
        .iter()
        .filter_map(|id| items.remove(id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Collection, FakeMediaSource, MediaId, MediaKind};

    fn episode(id: &str) -> MediaItem {
        MediaItem::new(id, id, MediaKind::Episode, 1_320_000)
    }

    fn library() -> FakeMediaSource {
        FakeMediaSource::new()
            .with_item(episode("a"))
            .with_item(episode("b"))
            .with_item(episode("c"))
    }

    fn ids(pool: &[MediaItem]) -> Vec<&str> {
        pool.iter().map(|item| item.id.as_str()).collect()
    }

    fn collection(name: &str) -> Source {
        Source::JellyfinCollection(name.into())
    }

    #[test]
    fn a_collection_resolves_in_member_order_not_item_order() {
        let media = library().with_collection("Cartoons", &["c", "a", "b"]);

        let pool = resolve_pool(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["c", "a", "b"]);
        assert_eq!(pool[0], episode("c"));
    }

    #[test]
    fn a_member_the_source_does_not_list_is_skipped() {
        let media = library().with_collection("Cartoons", &["b", "gone", "a"]);

        let pool = resolve_pool(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b", "a"]);
    }

    #[test]
    fn a_repeated_member_appears_once_at_its_first_place() {
        let media = library().with_collection("Cartoons", &["b", "a", "b"]);

        let pool = resolve_pool(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b", "a"]);
    }

    #[test]
    fn the_first_collection_with_the_exact_name_wins() {
        let media = library()
            .with_collection("cartoons", &["a"])
            .with_collection("Cartoons", &["b"])
            .with_collection("Cartoons", &["c"]);

        let pool = resolve_pool(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b"]);
    }

    #[test]
    fn an_empty_collection_gives_an_empty_pool() {
        let media = library().with_collection("Empty", &[]);

        assert_eq!(resolve_pool(&collection("Empty"), &media), Ok(Vec::new()));
    }

    #[test]
    fn an_unknown_collection_is_not_found() {
        let media = library().with_collection("Cartoons", &["a"]);

        let err = resolve_pool(&collection("Westerns"), &media).unwrap_err();

        assert_eq!(err, PoolError::CollectionNotFound("Westerns".into()));
        assert_eq!(err.to_string(), "no collection named \"Westerns\"");
    }

    #[test]
    fn libraries_tags_genres_and_series_are_unsupported_not_every_item() {
        let media = library().with_collection("Movies", &["a"]);
        let sources = [
            Source::JellyfinLibrary("Movies".into()),
            Source::JellyfinTag("Comedy".into()),
            Source::JellyfinGenre("Drama".into()),
            Source::JellyfinSeries("Cheers".into()),
        ];

        for source in sources {
            assert_eq!(
                resolve_pool(&source, &media),
                Err(PoolError::Unsupported(source.clone()))
            );
        }
        assert_eq!(
            PoolError::Unsupported(Source::JellyfinTag("Comedy".into())).to_string(),
            "jellyfin_tag \"Comedy\" cannot be resolved yet (#30)"
        );
    }

    /// A media source that is always down.
    struct Down;

    impl MediaSource for Down {
        fn list_items(&self) -> Result<Vec<MediaItem>, SourceError> {
            Err(SourceError::Unavailable("down".into()))
        }

        fn resolve(&self, _: &MediaId) -> Result<MediaItem, SourceError> {
            Err(SourceError::Unavailable("down".into()))
        }

        fn list_collections(&self) -> Result<Vec<Collection>, SourceError> {
            Err(SourceError::Unavailable("down".into()))
        }
    }

    #[test]
    fn a_failing_source_is_reported() {
        let err = resolve_pool(&collection("Cartoons"), &Down).unwrap_err();

        assert_eq!(
            err,
            PoolError::Source(SourceError::Unavailable("down".into()))
        );
        assert_eq!(
            err.to_string(),
            "media source: media source unavailable: down"
        );
        assert!(Error::source(&err).is_some());
    }
}
