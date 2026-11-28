//! Album covers for the cover card, fetched off the loop's thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::jellyfin::JellyfinClient;
use crate::library::MediaId;
use crate::paint::Image;

/// The side, in pixels, a cover is asked for: the card's cover box.
pub const COVER_SIDE: u32 = 440;

/// Where the loop gets album covers. [`Covers::request`] returns at once;
/// the cover comes back later from [`Covers::take`], so the loop never waits
/// on the network.
pub trait Covers {
    /// Asks for the primary image of `id`: a song's album id, or the song's
    /// own id when it has no album.
    fn request(&mut self, id: &MediaId);

    /// The covers that arrived since the last call. `None` is a cover that
    /// does not exist or could not be read; the card then shows the mark.
    fn take(&mut self) -> Vec<(MediaId, Option<Image>)>;
}

/// [`Covers`] from Jellyfin, `GET /Items/{id}/Images/Primary` as a PNG, on a
/// thread named `jellyfin-covers`. Dropping it stops the thread once the
/// request in progress ends.
pub struct JellyfinCovers {
    requests: Sender<MediaId>,
    results: Receiver<(MediaId, Option<Image>)>,
}

impl JellyfinCovers {
    pub fn spawn(client: JellyfinClient) -> std::io::Result<Self> {
        let (requests, wanted) = mpsc::channel::<MediaId>();
        let (found, results) = mpsc::channel();
        thread::Builder::new()
            .name("jellyfin-covers".to_string())
            .spawn(move || {
                for id in wanted {
                    let image = match client.primary_image(&id, COVER_SIDE) {
                        Ok(bytes) => Image::decode_png(&bytes),
                        Err(e) => {
                            eprintln!("surfcult: no cover for {id}: {e}");
                            None
                        }
                    };
                    if found.send((id, image)).is_err() {
                        return;
                    }
                }
            })?;
        Ok(Self { requests, results })
    }
}

impl Covers for JellyfinCovers {
    fn request(&mut self, id: &MediaId) {
        let _ = self.requests.send(id.clone());
    }

    fn take(&mut self) -> Vec<(MediaId, Option<Image>)> {
        self.results.try_iter().collect()
    }
}
