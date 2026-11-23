//! The media library as the rest of surfcult sees it.
//!
//! [`MediaItem`] describes one playable item. [`MediaSource`] lists and
//! resolves items without saying where they come from: Jellyfin is the first
//! real source, and [`FakeMediaSource`] stands in for it in tests. The trait is
//! synchronous; a network-backed source can run on a blocking thread.

mod fake;
mod model;
mod source;

pub use fake::FakeMediaSource;
pub use model::{AudioStream, Chapter, Collection, MediaId, MediaItem, MediaKind, VideoStream};
pub use source::{MediaSource, SourceError};
