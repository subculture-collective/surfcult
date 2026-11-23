use super::model::{Collection, MediaId, MediaItem};
use super::source::{MediaSource, SourceError};

/// An in-memory [`MediaSource`] filled by the caller, for tests.
#[derive(Debug, Default, Clone)]
pub struct FakeMediaSource {
    items: Vec<MediaItem>,
    collections: Vec<Collection>,
}

impl FakeMediaSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_item(mut self, item: MediaItem) -> Self {
        self.items.push(item);
        self
    }

    /// Adds a collection of items already added with [`Self::with_item`].
    pub fn with_collection(mut self, name: impl Into<String>, members: &[&str]) -> Self {
        self.collections.push(Collection {
            name: name.into(),
            members: members.iter().map(|id| MediaId::new(*id)).collect(),
        });
        self
    }
}

impl MediaSource for FakeMediaSource {
    fn list_items(&self) -> Result<Vec<MediaItem>, SourceError> {
        Ok(self.items.clone())
    }

    fn resolve(&self, id: &MediaId) -> Result<MediaItem, SourceError> {
        self.items
            .iter()
            .find(|item| &item.id == id)
            .cloned()
            .ok_or_else(|| SourceError::NotFound(id.clone()))
    }

    fn list_collections(&self) -> Result<Vec<Collection>, SourceError> {
        Ok(self.collections.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{AudioStream, Chapter, MediaKind, VideoStream};

    fn episode(id: &str, minutes: u64) -> MediaItem {
        MediaItem::new(
            id,
            format!("Episode {id}"),
            MediaKind::Episode,
            minutes * 60_000,
        )
    }

    #[test]
    fn item_carries_runtime_chapters_and_streams() {
        let item = MediaItem {
            chapters: vec![
                Chapter {
                    start_ms: 0,
                    name: None,
                },
                Chapter {
                    start_ms: 660_000,
                    name: Some("Act 2".into()),
                },
            ],
            video: vec![VideoStream {
                codec: "hevc".into(),
                width: 1920,
                height: 1080,
                bit_depth: Some(10),
            }],
            audio: vec![AudioStream {
                codec: "eac3".into(),
                channels: 6,
                language: Some("eng".into()),
            }],
            ..episode("a", 22)
        };
        let source = FakeMediaSource::new().with_item(item.clone());

        let found = source.resolve(&MediaId::new("a")).unwrap();

        assert_eq!(found, item);
        assert_eq!(found.runtime_ms, 1_320_000);
        assert_eq!(found.chapters[1].start_ms, 660_000);
        assert_eq!(found.video[0].codec, "hevc");
        assert_eq!(found.audio[0].channels, 6);
    }

    #[test]
    fn lists_items_in_insertion_order() {
        let source = FakeMediaSource::new()
            .with_item(episode("b", 22))
            .with_item(episode("a", 22));

        let ids: Vec<_> = source
            .list_items()
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();

        assert_eq!(ids, vec![MediaId::new("b"), MediaId::new("a")]);
    }

    #[test]
    fn collections_list_their_members() {
        let source = FakeMediaSource::new()
            .with_item(episode("a", 22))
            .with_item(episode("b", 22))
            .with_collection("Cartoons", &["a", "b"]);

        let collections = source.list_collections().unwrap();

        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "Cartoons");
        assert_eq!(
            collections[0].members,
            vec![MediaId::new("a"), MediaId::new("b")]
        );
    }

    #[test]
    fn resolving_an_unknown_id_is_not_found() {
        let source = FakeMediaSource::new().with_item(episode("a", 22));

        let err = source.resolve(&MediaId::new("missing")).unwrap_err();

        assert_eq!(err, SourceError::NotFound(MediaId::new("missing")));
        assert_eq!(err.to_string(), "media item missing not found");
    }
}
