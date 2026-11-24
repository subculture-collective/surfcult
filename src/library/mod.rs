//! The media library as the rest of surfcult sees it.
//!
//! [`MediaItem`] describes one playable item. [`MediaSource`] lists and
//! resolves items without saying where they come from. The real source is the
//! library cache in the `Store`, which the Jellyfin sync step fills;
//! [`FakeMediaSource`] stands in for it in tests. The trait is synchronous.

mod fake;
mod model;
pub mod profile;
mod source;

pub use fake::FakeMediaSource;
pub use model::{AudioStream, Chapter, Collection, MediaId, MediaItem, MediaKind, VideoStream};
pub use source::{MediaSource, SourceError};
