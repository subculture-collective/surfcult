use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use super::model::{MediaId, MediaItem, Plays};
use super::source::{ItemQuery, Label, MediaSource, SourceError};
use crate::config::Source;

/// Why a channel's source could not be turned into a program pool. It names
/// the channel and the source, so the message can be shown as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolError {
    /// The channel number the caller passed.
    pub channel: u32,
    /// What the channel plays, so a song channel's error says it found no song.
    pub plays: Plays,
    pub source: Source,
    pub reason: PoolErrorReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolErrorReason {
    /// Nothing in the media source matches the source: no collection has the
    /// name, or no item is in the library, has the tag or genre, or belongs to
    /// the series.
    NotFound,
    /// The media source failed.
    Source(SourceError),
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (key, name) = match &self.source {
            Source::JellyfinLibrary(name) => ("jellyfin_library", name),
            Source::JellyfinCollection(name) => ("jellyfin_collection", name),
            Source::JellyfinTag(name) => ("jellyfin_tag", name),
            Source::JellyfinGenre(name) => ("jellyfin_genre", name),
            Source::JellyfinSeries(name) => ("jellyfin_series", name),
        };
        write!(f, "channel {}: {key} {name:?} ", self.channel)?;
        match &self.reason {
            PoolErrorReason::NotFound => match self.plays {
                Plays::Videos => f.write_str("matches nothing in the library"),
                Plays::Songs => f.write_str("matches no song in the library"),
            },
            PoolErrorReason::Source(error) => write!(f, "cannot be read: {error}"),
        }
    }
}

impl Error for PoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.reason {
            PoolErrorReason::Source(error) => Some(error),
            PoolErrorReason::NotFound => None,
        }
    }
}

/// Turns `source`, configured on channel `channel`, into its program pool:
/// the items to schedule, in the order
/// [`schedule::extend`](crate::schedule::extend) expects. Names match
/// exactly, including case.
///
/// Only items a channel playing `plays` may schedule are considered
/// ([`MediaKind::played_by`](super::MediaKind::played_by)): a video channel
/// never gets a song, even through a genre songs share, and a song channel
/// gets only songs.
///
/// - A collection is the first collection named `name`. Its members come in
///   collection order. A member that is a series becomes that series'
///   episodes in episode order; a movie or episode stays itself. A member the
///   media source does not list is skipped. An empty collection gives an
///   empty pool, which `extend` reports as `Note::NoPlayableItems`.
/// - A library, tag or genre is every item in that library or with that tag
///   or genre, in the media source's order, except that the episodes of one
///   series are gathered, in episode order, at the place of the series' first
///   matching episode.
/// - A series is the episodes of the first series, in the media source's
///   order, whose name is `name`, in episode order.
///
/// Episode order is season, then episode number; a missing number sorts
/// after the numbered ones, and ties keep the media source's order. Season 0
/// (Jellyfin's specials) comes first. An item reached twice, such as an
/// episode a collection lists both directly and through its series, appears
/// once, at its first place.
///
/// A library, tag, genre or series that matches no item, and a collection
/// that does not exist, is [`PoolErrorReason::NotFound`], so a channel never
/// silently falls back to the whole library.
pub fn resolve_pool(
    channel: u32,
    plays: Plays,
    source: &Source,
    media: &impl MediaSource,
) -> Result<Vec<MediaItem>, PoolError> {
    let error = |reason| PoolError {
        channel,
        plays,
        source: source.clone(),
        reason,
    };
    match pool_for(plays, source, media) {
        Ok(Some(pool)) => Ok(pool),
        Ok(None) => Err(error(PoolErrorReason::NotFound)),
        Err(source) => Err(error(PoolErrorReason::Source(source))),
    }
}

/// The pool for `source`, or `None` when nothing matches it.
fn pool_for(
    plays: Plays,
    source: &Source,
    media: &impl MediaSource,
) -> Result<Option<Vec<MediaItem>>, SourceError> {
    let all = ItemQuery { plays, label: None };
    let labelled = |label| ItemQuery {
        plays,
        label: Some(label),
    };
    let pool = match source {
        Source::JellyfinCollection(name) => {
            let Some(collection) = media
                .list_collections()?
                .into_iter()
                .find(|collection| &collection.name == name)
            else {
                return Ok(None);
            };
            return Ok(Some(expand(&collection.members, media.query_items(&all)?)));
        }
        Source::JellyfinLibrary(name) => matching(media, labelled(Label::Library(name)))?,
        Source::JellyfinTag(name) => matching(media, labelled(Label::Tag(name)))?,
        Source::JellyfinGenre(name) => matching(media, labelled(Label::Genre(name)))?,
        Source::JellyfinSeries(name) => {
            let items = media.query_items(&all)?;
            let Some(series) = items.iter().find_map(|item| {
                item.series
                    .as_ref()
                    .filter(|series| &series.series_name == name)
                    .map(|series| series.series_id.clone())
            }) else {
                return Ok(None);
            };
            gather_series(
                items
                    .into_iter()
                    .filter(|item| series_of(item) == Some(&series)),
            )
        }
    };
    Ok((!pool.is_empty()).then_some(pool))
}

/// The items `query` matches, with each series gathered by [`gather_series`].
fn matching(media: &impl MediaSource, query: ItemQuery<'_>) -> Result<Vec<MediaItem>, SourceError> {
    Ok(gather_series(media.query_items(&query)?.into_iter()))
}

fn series_of(item: &MediaItem) -> Option<&MediaId> {
    item.series.as_ref().map(|series| &series.series_id)
}

/// Sorts episodes into episode order. The sort is stable, so ties keep their
/// order.
fn sort_episodes(episodes: &mut [MediaItem]) {
    episodes.sort_by_key(|item| {
        let series = item.series.as_ref();
        let season = series.and_then(|s| s.season);
        let number = series.and_then(|s| s.number);
        (season.is_none(), season, number.is_none(), number)
    });
}

/// Keeps `items` in their order, except that each series' episodes are moved
/// together, in episode order, to where its first episode was.
fn gather_series(items: impl Iterator<Item = MediaItem>) -> Vec<MediaItem> {
    enum Slot {
        Item(Box<MediaItem>),
        Series(MediaId),
    }
    let mut slots = Vec::new();
    let mut episodes: HashMap<MediaId, Vec<MediaItem>> = HashMap::new();
    for item in items {
        match series_of(&item).cloned() {
            None => slots.push(Slot::Item(Box::new(item))),
            Some(series) => {
                let list = episodes.entry(series.clone()).or_default();
                if list.is_empty() {
                    slots.push(Slot::Series(series));
                }
                list.push(item);
            }
        }
    }
    let mut pool = Vec::new();
    for slot in slots {
        match slot {
            Slot::Item(item) => pool.push(*item),
            Slot::Series(series) => {
                let mut list = episodes.remove(&series).unwrap_or_default();
                sort_episodes(&mut list);
                pool.extend(list);
            }
        }
    }
    pool
}

/// A collection's members as items: an item id is that item, a series id is
/// the series' episodes in episode order. Each item is taken once.
fn expand(members: &[MediaId], items: Vec<MediaItem>) -> Vec<MediaItem> {
    let mut series: HashMap<MediaId, Vec<MediaItem>> = HashMap::new();
    for item in &items {
        if let Some(id) = series_of(item) {
            series.entry(id.clone()).or_default().push(item.clone());
        }
    }
    for episodes in series.values_mut() {
        sort_episodes(episodes);
    }
    let by_id: HashMap<MediaId, MediaItem> = items
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();

    let mut taken = HashSet::new();
    let mut pool = Vec::new();
    for id in members {
        let found = match by_id.get(id) {
            Some(item) => std::slice::from_ref(item),
            None => series.get(id).map_or(&[][..], Vec::as_slice),
        };
        for item in found {
            if taken.insert(item.id.clone()) {
                pool.push(item.clone());
            }
        }
    }
    pool
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Collection, FakeMediaSource, MediaKind, SeriesEpisode};

    const CHANNEL: u32 = 3;

    fn episode(id: &str) -> MediaItem {
        MediaItem::new(id, id, MediaKind::Episode, 1_320_000)
    }

    fn movie(id: &str) -> MediaItem {
        MediaItem::new(id, id, MediaKind::Movie, 5_400_000)
    }

    /// An episode of `series`, which is also the series' name.
    fn of(series: &str, id: &str, season: Option<u32>, number: Option<u32>) -> MediaItem {
        MediaItem {
            series: Some(SeriesEpisode {
                series_id: MediaId::new(series),
                series_name: series.to_string(),
                season,
                number,
            }),
            ..episode(id)
        }
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

    fn resolve(source: &Source, media: &impl MediaSource) -> Result<Vec<MediaItem>, PoolError> {
        resolve_pool(CHANNEL, Plays::Videos, source, media)
    }

    fn not_found(source: Source) -> PoolError {
        PoolError {
            channel: CHANNEL,
            plays: Plays::Videos,
            source,
            reason: PoolErrorReason::NotFound,
        }
    }

    // ---- collections ----------------------------------------------------

    #[test]
    fn a_collection_resolves_in_member_order_not_item_order() {
        let media = library().with_collection("Cartoons", &["c", "a", "b"]);

        let pool = resolve(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["c", "a", "b"]);
        assert_eq!(pool[0], episode("c"));
    }

    #[test]
    fn a_member_the_source_does_not_list_is_skipped() {
        let media = library().with_collection("Cartoons", &["b", "gone", "a"]);

        let pool = resolve(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b", "a"]);
    }

    #[test]
    fn a_repeated_member_appears_once_at_its_first_place() {
        let media = library().with_collection("Cartoons", &["b", "a", "b"]);

        let pool = resolve(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b", "a"]);
    }

    #[test]
    fn the_first_collection_with_the_exact_name_wins() {
        let media = library()
            .with_collection("cartoons", &["a"])
            .with_collection("Cartoons", &["b"])
            .with_collection("Cartoons", &["c"]);

        let pool = resolve(&collection("Cartoons"), &media).unwrap();

        assert_eq!(ids(&pool), ["b"]);
    }

    #[test]
    fn an_empty_collection_gives_an_empty_pool() {
        let media = library().with_collection("Empty", &[]);

        assert_eq!(resolve(&collection("Empty"), &media), Ok(Vec::new()));
    }

    #[test]
    fn a_collection_of_series_resolves_to_all_their_episodes_and_keeps_movies() {
        let media = FakeMediaSource::new()
            // Sync order is by name, not by episode.
            .with_item(of("Cheers", "cheers-2x1", Some(2), Some(1)))
            .with_item(of("Frasier", "frasier-1x1", Some(1), Some(1)))
            .with_item(movie("film"))
            .with_item(of("Cheers", "cheers-1x2", Some(1), Some(2)))
            .with_item(of("Cheers", "cheers-1x1", Some(1), Some(1)))
            .with_collection("Sitcoms", &["Cheers", "film", "Frasier"]);

        let pool = resolve(&collection("Sitcoms"), &media).unwrap();

        assert_eq!(
            ids(&pool),
            [
                "cheers-1x1",
                "cheers-1x2",
                "cheers-2x1",
                "film",
                "frasier-1x1"
            ]
        );
    }

    #[test]
    fn an_episode_listed_directly_and_through_its_series_appears_once() {
        let media = FakeMediaSource::new()
            .with_item(of("Cheers", "e1", Some(1), Some(1)))
            .with_item(of("Cheers", "e2", Some(1), Some(2)))
            .with_collection("Picks", &["e2", "Cheers"]);

        let pool = resolve(&collection("Picks"), &media).unwrap();

        assert_eq!(ids(&pool), ["e2", "e1"]);
    }

    // ---- libraries, tags and genres -------------------------------------

    #[test]
    fn a_library_resolves_to_its_items_with_each_series_gathered_in_episode_order() {
        let in_tv = |mut item: MediaItem| {
            item.libraries = vec!["TV".into()];
            item
        };
        let media = FakeMediaSource::new()
            .with_item(in_tv(of("Cheers", "c2", Some(1), Some(2))))
            .with_item(in_tv(episode("loose")))
            .with_item(movie("elsewhere"))
            .with_item(in_tv(of("Taxi", "t1", Some(1), Some(1))))
            .with_item(in_tv(of("Cheers", "c1", Some(1), Some(1))));

        let pool = resolve(&Source::JellyfinLibrary("TV".into()), &media).unwrap();

        assert_eq!(ids(&pool), ["c1", "c2", "loose", "t1"]);
    }

    #[test]
    fn a_tag_resolves_to_the_items_with_that_tag() {
        let tagged = |id: &str, tags: &[&str]| MediaItem {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..movie(id)
        };
        let media = FakeMediaSource::new()
            .with_item(tagged("a", &["Saturday", "Retro"]))
            .with_item(tagged("b", &["saturday"]))
            .with_item(tagged("c", &["Retro"]))
            .with_item(tagged("d", &[]));

        let pool = resolve(&Source::JellyfinTag("Retro".into()), &media).unwrap();

        assert_eq!(ids(&pool), ["a", "c"]);
    }

    #[test]
    fn a_genre_resolves_to_the_items_in_that_genre() {
        let genre = |mut item: MediaItem, genre: &str| {
            item.genres = vec![genre.into()];
            item
        };
        let media = FakeMediaSource::new()
            .with_item(genre(movie("western"), "Western"))
            .with_item(genre(of("Bonanza", "b2", Some(1), Some(2)), "Western"))
            .with_item(genre(movie("comedy"), "Comedy"))
            .with_item(genre(of("Bonanza", "b1", Some(1), Some(1)), "Western"));

        let pool = resolve(&Source::JellyfinGenre("Western".into()), &media).unwrap();

        assert_eq!(ids(&pool), ["western", "b1", "b2"]);
    }

    // ---- series ---------------------------------------------------------

    #[test]
    fn a_series_resolves_to_its_episodes_in_season_and_episode_order() {
        let media = FakeMediaSource::new()
            .with_item(of("Cheers", "unnumbered", Some(1), None))
            .with_item(of("Cheers", "s2e1", Some(2), Some(1)))
            .with_item(of("Cheers", "s1e10", Some(1), Some(10)))
            .with_item(of("Frasier", "other", Some(1), Some(1)))
            .with_item(of("Cheers", "s1e2", Some(1), Some(2)))
            .with_item(of("Cheers", "special", Some(0), Some(1)))
            .with_item(of("Cheers", "no-season", None, Some(1)));

        let pool = resolve(&Source::JellyfinSeries("Cheers".into()), &media).unwrap();

        assert_eq!(
            ids(&pool),
            [
                "special",
                "s1e2",
                "s1e10",
                "unnumbered",
                "s2e1",
                "no-season"
            ]
        );
    }

    #[test]
    fn of_two_series_with_one_name_the_first_listed_wins() {
        let remake = |id: &str| MediaItem {
            series: Some(SeriesEpisode {
                series_id: MediaId::new("remake"),
                series_name: "Cheers".into(),
                season: Some(1),
                number: Some(1),
            }),
            ..episode(id)
        };
        let media = FakeMediaSource::new()
            .with_item(remake("new"))
            .with_item(of("Cheers", "old", Some(1), Some(1)));

        let pool = resolve(&Source::JellyfinSeries("Cheers".into()), &media).unwrap();

        assert_eq!(ids(&pool), ["new"]);
    }

    // ---- errors ---------------------------------------------------------

    #[test]
    fn an_unknown_collection_is_not_found() {
        let media = library().with_collection("Cartoons", &["a"]);

        let err = resolve(&collection("Westerns"), &media).unwrap_err();

        assert_eq!(err, not_found(collection("Westerns")));
        assert_eq!(
            err.to_string(),
            "channel 3: jellyfin_collection \"Westerns\" matches nothing in the library"
        );
    }

    #[test]
    fn an_unknown_source_of_any_kind_names_the_channel_and_the_source() {
        let media = FakeMediaSource::new()
            .with_item(MediaItem {
                libraries: vec!["Movies".into()],
                tags: vec!["Comedy".into()],
                genres: vec!["Drama".into()],
                ..of("Cheers", "a", Some(1), Some(1))
            })
            .with_collection("Movies", &["a"]);
        let cases = [
            (
                Source::JellyfinLibrary("Shows".into()),
                "jellyfin_library \"Shows\"",
            ),
            (
                Source::JellyfinTag("comedy".into()),
                "jellyfin_tag \"comedy\"",
            ),
            (
                Source::JellyfinGenre("Horror".into()),
                "jellyfin_genre \"Horror\"",
            ),
            (
                Source::JellyfinSeries("Taxi".into()),
                "jellyfin_series \"Taxi\"",
            ),
        ];

        for (source, named) in cases {
            let err = resolve(&source, &media).unwrap_err();
            assert_eq!(err, not_found(source.clone()));
            assert_eq!(
                err.to_string(),
                format!("channel 3: {named} matches nothing in the library")
            );
        }
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
    fn a_failing_source_is_reported_for_every_kind() {
        for source in [
            collection("Cartoons"),
            Source::JellyfinLibrary("TV".into()),
            Source::JellyfinTag("Retro".into()),
            Source::JellyfinGenre("Drama".into()),
            Source::JellyfinSeries("Cheers".into()),
        ] {
            let err = resolve(&source, &Down).unwrap_err();
            assert_eq!(
                err.reason,
                PoolErrorReason::Source(SourceError::Unavailable("down".into()))
            );
            assert!(Error::source(&err).is_some());
        }
        assert_eq!(
            resolve(&collection("Cartoons"), &Down)
                .unwrap_err()
                .to_string(),
            "channel 3: jellyfin_collection \"Cartoons\" cannot be read: \
             media source unavailable: down"
        );
    }

    // ---- songs ----------------------------------------------------------

    fn song(id: &str, genre: &str) -> MediaItem {
        MediaItem {
            genres: vec![genre.into()],
            libraries: vec!["Music".into()],
            ..MediaItem::new(id, id, MediaKind::Song, 240_000)
        }
    }

    fn mixed() -> FakeMediaSource {
        FakeMediaSource::new()
            .with_item(MediaItem {
                genres: vec!["Comedy".into()],
                ..movie("film")
            })
            .with_item(song("tune", "Comedy"))
            .with_item(song("punk", "Punk"))
            .with_collection("Mixed", &["film", "tune"])
    }

    #[test]
    fn a_video_channel_never_gets_a_song_through_a_shared_genre() {
        let media = mixed();
        let genre = |name: &str| Source::JellyfinGenre(name.into());

        assert_eq!(ids(&resolve(&genre("Comedy"), &media).unwrap()), ["film"]);
        assert_eq!(
            resolve(&genre("Punk"), &media).unwrap_err(),
            not_found(genre("Punk"))
        );
        assert_eq!(
            ids(&resolve(&collection("Mixed"), &media).unwrap()),
            ["film"]
        );
    }

    #[test]
    fn a_song_channel_gets_only_songs() {
        let media = mixed();
        let songs = |source: Source| resolve_pool(CHANNEL, Plays::Songs, &source, &media);

        assert_eq!(
            ids(&songs(Source::JellyfinGenre("Comedy".into())).unwrap()),
            ["tune"]
        );
        assert_eq!(
            ids(&songs(Source::JellyfinLibrary("Music".into())).unwrap()),
            ["tune", "punk"]
        );
        assert_eq!(ids(&songs(collection("Mixed")).unwrap()), ["tune"]);
        let err = songs(Source::JellyfinTag("Retro".into())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "channel 3: jellyfin_tag \"Retro\" matches no song in the library"
        );
    }
}
