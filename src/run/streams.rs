//! Programs from Jellyfin for MPV: the stream resolver and the transcodes it
//! started.

use std::cell::RefCell;
use std::rc::Rc;

use crate::jellyfin::{JellyfinClient, JellyfinConfig, Playback, TranscodeSession};
use crate::library::MediaId;
use crate::player::PlayerError;
use crate::player::mpv::{Stream, StreamResolver};

/// What the loop can do about the streams the player opened.
pub trait Streams {
    /// Stops every transcode started for a program that is leaving the
    /// screen. Best effort: errors are logged, never returned.
    fn stop_all(&mut self);
}

/// The [`StreamResolver`] that asks Jellyfin how to play each program, and
/// the [`Streams`] that stops the transcodes it started. They share the list
/// of open transcode sessions.
pub fn jellyfin(config: &JellyfinConfig) -> (JellyfinResolver, Transcodes) {
    jellyfin_with(JellyfinClient::new(config.clone()), config)
}

/// [`jellyfin`] with a given client, for tests.
pub(super) fn jellyfin_with(
    client: JellyfinClient,
    config: &JellyfinConfig,
) -> (JellyfinResolver, Transcodes) {
    let client = Rc::new(client);
    let sessions = Rc::new(RefCell::new(Vec::new()));
    let resolver = JellyfinResolver {
        client: client.clone(),
        base_url: config.base_url.clone(),
        authorization: config.key.authorization_header(),
        sessions: sessions.clone(),
    };
    (resolver, Transcodes { client, sessions })
}

/// Turns a program into a [`Stream`] with [`JellyfinClient::playback`]: the
/// URL, the `Authorization: MediaBrowser Token="..."` header (the key goes
/// nowhere else: not in the URL, MPV's arguments or a log), and where the
/// stream begins. A direct-play file begins at the start of the media, so
/// MPV seeks; a transcode begins at the start position. A transcode's
/// session is kept so [`Transcodes::stop_all`] can stop it.
pub struct JellyfinResolver {
    client: Rc<JellyfinClient>,
    base_url: String,
    authorization: String,
    sessions: Rc<RefCell<Vec<TranscodeSession>>>,
}

impl StreamResolver for JellyfinResolver {
    fn stream(&self, media: &MediaId, start_ms: u64) -> Result<Stream, PlayerError> {
        let playback = self
            .client
            .playback(media, start_ms)
            .map_err(|e| PlayerError(format!("Jellyfin cannot play {media}: {e}")))?;
        let mut stream = Stream::new(playback.url(&self.base_url));
        stream
            .headers
            .push(("Authorization".into(), self.authorization.clone()));
        if let Playback::Transcode { session, .. } = &playback {
            stream.stream_offset_ms = start_ms;
            self.sessions.borrow_mut().push(session.clone());
        }
        Ok(stream)
    }
}

/// The transcodes [`JellyfinResolver`] started and has not stopped.
pub struct Transcodes {
    client: Rc<JellyfinClient>,
    sessions: Rc<RefCell<Vec<TranscodeSession>>>,
}

impl Streams for Transcodes {
    fn stop_all(&mut self) {
        let sessions: Vec<TranscodeSession> = self.sessions.borrow_mut().drain(..).collect();
        for session in sessions {
            if let Err(e) = self.client.stop_transcode(&session) {
                eprintln!(
                    "surfcult: cannot stop the Jellyfin transcode {}: {e}",
                    session.play_session_id
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::jellyfin::{ApiKey, HttpError, JellyfinHttp};

    const DIRECT_ID: &str = "1e0ff4a8a553696127930e218769b7b7";
    const TRANSCODE_ID: &str = "3282307ad80560f83e46a3022f90087e";
    const KEY: &str = "SENTINEL-KEY-0123456789abcdef";

    /// Answers `PlaybackInfo` with the recorded fixtures and records every
    /// `DELETE`.
    #[derive(Clone, Default)]
    struct Fake {
        deleted: Arc<Mutex<Vec<String>>>,
    }

    impl JellyfinHttp for Fake {
        fn get(&self, _: &str, _: &str) -> Result<String, HttpError> {
            Err(HttpError::Unreachable("not in this test".into()))
        }

        fn post_json(&self, path: &str, _: &str, auth: &str) -> Result<String, HttpError> {
            assert_eq!(auth, format!("MediaBrowser Token=\"{KEY}\""));
            let fixture = if path.contains(TRANSCODE_ID) {
                "playback-info-transcode.json"
            } else {
                "playback-info-direct.json"
            };
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jellyfin/");
            Ok(std::fs::read_to_string(format!("{dir}{fixture}")).unwrap())
        }

        fn delete(&self, path: &str, _: &str) -> Result<(), HttpError> {
            self.deleted.lock().unwrap().push(path.to_string());
            Ok(())
        }
    }

    fn rig() -> (JellyfinResolver, Transcodes, Fake) {
        let fake = Fake::default();
        let key = ApiKey::new(KEY).unwrap();
        let config = JellyfinConfig {
            base_url: "http://jellyfin:8096/".into(),
            key: key.clone(),
            user_id: Some("user".into()),
        };
        let client = JellyfinClient::with_transport(key, fake.clone()).with_user_id("user");
        let (resolver, transcodes) = jellyfin_with(client, &config);
        (resolver, transcodes, fake)
    }

    #[test]
    fn a_direct_stream_carries_the_key_only_in_its_header_and_begins_at_zero() {
        let (resolver, mut transcodes, fake) = rig();
        let stream = resolver.stream(&MediaId::new(DIRECT_ID), 600_000).unwrap();
        assert!(stream.url.starts_with("http://jellyfin:8096/Videos/"));
        assert!(!stream.url.contains(KEY));
        assert_eq!(
            stream.headers,
            [(
                "Authorization".to_string(),
                format!("MediaBrowser Token=\"{KEY}\"")
            )]
        );
        assert_eq!(stream.stream_offset_ms, 0);
        assert!(!format!("{stream:?}").contains(KEY));
        transcodes.stop_all();
        assert!(fake.deleted.lock().unwrap().is_empty());
    }

    #[test]
    fn a_transcode_begins_at_its_start_and_is_stopped_once() {
        let (resolver, mut transcodes, fake) = rig();
        let stream = resolver
            .stream(&MediaId::new(TRANSCODE_ID), 600_000)
            .unwrap();
        assert!(stream.url.contains("/stream.ts?"));
        assert!(!stream.url.contains(KEY));
        assert_eq!(stream.stream_offset_ms, 600_000);
        transcodes.stop_all();
        transcodes.stop_all();
        let deleted = fake.deleted.lock().unwrap().clone();
        assert_eq!(deleted.len(), 1);
        assert!(deleted[0].starts_with("/Videos/ActiveEncodings?DeviceId=surfcult&PlaySessionId="));
    }
}
