//! The media library as the rest of surfcult sees it.
//!
//! [`MediaItem`] describes one playable item. [`MediaSource`] lists and
//! resolves items without saying where they come from. The real source is the
//! library cache in the `Store`, which the Jellyfin sync step fills;
//! [`FakeMediaSource`] stands in for it in tests. The trait is synchronous.
//! [`resolve_pool`] turns a channel's configured source into its program pool.

mod fake;
mod model;
mod pool;
pub mod profile;
mod source;

pub use fake::FakeMediaSource;
pub use model::{AudioStream, Chapter, Collection, MediaId, MediaItem, MediaKind, VideoStream};
pub use pool::{PoolError, resolve_pool};
pub use source::{MediaSource, SourceError};
