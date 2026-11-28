//! Asking Jellyfin how to play an item, and stopping a transcode surfcult
//! started.
//!
//! [`JellyfinClient::playback`] sends `POST /Items/{id}/PlaybackInfo` with the
//! device profile built from [`crate::library::profile`] and returns a
//! [`Playback`]: the file as stored when the Pi plays it directly, or a
//! progressive MPEG-TS transcode that starts at the requested position.
//! [`JellyfinClient::stop_transcode`] ends that transcode with
//! `DELETE /Videos/ActiveEncodings`.
//!
//! Nothing here reports playback: no request goes to a `/Sessions/Playing`
//! endpoint, so the owner's watched and continue-watching lists stay as they
//! are. The URLs carry no key; the player must send the `Authorization`
//! header itself.

use serde::Deserialize;
use serde_json::{Value, json};

use super::client::{JellyfinClient, JellyfinError, encode};
use super::wire::TICKS_PER_MS;
use crate::library::MediaId;
use crate::library::profile::{
    DIRECT_AUDIO, DIRECT_VIDEO, DIRECT_VIDEO_RANGES, MAX_DIRECT_HEIGHT, TRANSCODE,
};

/// The device id surfcult puts on its transcodes, so that it can stop them.
pub const DEVICE_ID: &str = "surfcult";

/// The bitrate ceiling in the playback-info request, in bits per second. It is
/// high enough that bitrate never rules out direct play on the LAN; the
/// transcode bitrate comes from [`TRANSCODE`] instead.
const MAX_STREAMING_BITRATE: u64 = 200_000_000;

/// Query parameters that carry a key. Jellyfin puts one in `TranscodingUrl`;
/// surfcult removes it, because the key goes only in the `Authorization` header.
const KEY_PARAMS: [&str; 2] = ["api_key", "ApiKey"];

/// How the player gets an item's picture and sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playback {
    /// The file as stored, `/Videos/{id}/stream?static=true&mediaSourceId=...`,
    /// or `/Audio/{id}/stream?...` for a source without a video stream (a
    /// song). The stream starts at the beginning, so the player seeks to
    /// `seek_ms`.
    Direct {
        path_and_query: String,
        seek_ms: u64,
    },
    /// A progressive MPEG-TS transcode, `/Videos/{id}/stream.ts?...`, that
    /// already starts at the requested position: the player must not seek.
    /// Stop it with [`JellyfinClient::stop_transcode`] when playback ends.
    Transcode {
        path_and_query: String,
        session: TranscodeSession,
    },
}

impl Playback {
    /// The path and query on the Jellyfin server. It holds no key.
    pub fn path_and_query(&self) -> &str {
        match self {
            Self::Direct { path_and_query, .. } | Self::Transcode { path_and_query, .. } => {
                path_and_query
            }
        }
    }

    /// The full URL on the server at `base_url`, such as
    /// `http://10.0.0.200:8096`. A trailing slash is ignored.
    pub fn url(&self, base_url: &str) -> String {
        format!(
            "{}{}",
            base_url.trim_end_matches('/'),
            self.path_and_query()
        )
    }

    /// The transcode to stop when playback ends, if there is one.
    pub fn transcode(&self) -> Option<&TranscodeSession> {
        match self {
            Self::Direct { .. } => None,
            Self::Transcode { session, .. } => Some(session),
        }
    }
}

/// What identifies one transcode to Jellyfin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeSession {
    pub device_id: String,
    pub play_session_id: String,
}

/// The part of Jellyfin's `PlaybackInfoResponse` surfcult reads.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawPlaybackInfo {
    media_sources: Option<Vec<RawMediaSource>>,
    play_session_id: Option<String>,
    error_code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawMediaSource {
    id: Option<String>,
    supports_direct_play: Option<bool>,
    transcoding_url: Option<String>,
    media_streams: Option<Vec<RawMediaStream>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawMediaStream {
    #[serde(rename = "Type")]
    kind: Option<String>,
}

impl RawMediaSource {
    /// Whether the source has a picture of its own. A song's cover art is an
    /// `EmbeddedImage` stream, not a `Video` one.
    fn has_video(&self) -> bool {
        self.media_streams
            .iter()
            .flatten()
            .any(|stream| stream.kind.as_deref() == Some("Video"))
    }
}

impl JellyfinClient {
    /// Asks Jellyfin how the Pi should play `id`, starting `start_ms` into it.
    ///
    /// Jellyfin decides between direct play and a transcode by comparing the
    /// item with the device profile. Subtitles are turned off, so they never
    /// force a transcode. Jellyfin's first media source is used.
    pub fn playback(&self, id: &MediaId, start_ms: u64) -> Result<Playback, JellyfinError> {
        let start_ticks = start_ms.saturating_mul(TICKS_PER_MS as u64);
        let request = json!({
            "UserId": self.user_id()?,
            "DeviceProfile": device_profile(),
            "MaxStreamingBitrate": MAX_STREAMING_BITRATE,
            "StartTimeTicks": start_ticks,
            "SubtitleStreamIndex": -1,
            "EnableDirectPlay": true,
            "EnableDirectStream": false,
            "EnableTranscoding": true,
            "AllowVideoStreamCopy": true,
            "AllowAudioStreamCopy": true,
            "AutoOpenLiveStream": false,
        });
        let body = self.post_json(
            &format!("/Items/{}/PlaybackInfo", encode(id.as_str())),
            &request.to_string(),
        )?;
        let info: RawPlaybackInfo = serde_json::from_str(&body)
            .map_err(|source| JellyfinError::BadResponse(self.scrub(&source.to_string())))?;
        if let Some(code) = info.error_code.filter(|code| !code.is_empty()) {
            return Err(JellyfinError::NoPlayback(self.scrub(&code)));
        }
        let Some(source) = info
            .media_sources
            .and_then(|sources| sources.into_iter().next())
        else {
            return Err(JellyfinError::NoPlayback("no media source".into()));
        };

        if source.supports_direct_play == Some(true) {
            let kind = if source.has_video() {
                "Videos"
            } else {
                "Audio"
            };
            let source_id = source.id.unwrap_or_else(|| id.as_str().to_string());
            return Ok(Playback::Direct {
                path_and_query: format!(
                    "/{kind}/{}/stream?static=true&mediaSourceId={}",
                    encode(id.as_str()),
                    encode(&source_id)
                ),
                seek_ms: start_ms,
            });
        }

        let Some(url) = source.transcoding_url.filter(|url| !url.is_empty()) else {
            return Err(JellyfinError::NoPlayback(
                "neither direct play nor a transcode".into(),
            ));
        };
        let Some(play_session_id) = info.play_session_id.filter(|id| !id.is_empty()) else {
            return Err(JellyfinError::BadResponse(
                "a transcode without a PlaySessionId".into(),
            ));
        };
        let session = TranscodeSession {
            device_id: DEVICE_ID.to_string(),
            play_session_id,
        };
        let path_and_query = transcode_url(&url, &session, start_ticks)
            .map_err(|reason| JellyfinError::BadResponse(reason.into()))?;
        let url_item = transcode_item_id(&path_and_query);
        if !same_item_id(url_item, id.as_str()) {
            return Err(JellyfinError::WrongItem(self.scrub(url_item)));
        }
        Ok(Playback::Transcode {
            path_and_query,
            session,
        })
    }

    /// Stops the transcode `session` names, so Jellyfin stops encoding for a
    /// stream nobody watches. Stopping one that already ended is not an error:
    /// Jellyfin answers 404 for a session it no longer has, and that counts as
    /// stopped.
    pub fn stop_transcode(&self, session: &TranscodeSession) -> Result<(), JellyfinError> {
        let stopped = self.delete(&format!(
            "/Videos/ActiveEncodings?DeviceId={}&PlaySessionId={}",
            encode(&session.device_id),
            encode(&session.play_session_id)
        ));
        match stopped {
            Err(JellyfinError::Status(404)) => Ok(()),
            other => other,
        }
    }
}

/// The `DeviceProfile` sent with every playback-info request, built from
/// [`crate::library::profile`] so the ADR's rules are written down once.
///
/// The direct-play entries name no container: any container plays directly,
/// as in `needs_transcode`. The `Audio` entry lets a song play directly; there
/// is no audio transcode, so a song in another codec cannot play. Each direct video codec gets a codec profile that
/// caps its bit depth and height and allows only SDR. The conditions are not
/// required, so a stream whose bit depth Jellyfin does not know still plays
/// directly, the same way `needs_transcode` reads a missing bit depth as 8.
pub(super) fn device_profile() -> Value {
    let video_codecs: Vec<&str> = DIRECT_VIDEO.iter().map(|rule| rule.names[0]).collect();
    let condition = |condition: &str, property: &str, value: String| {
        json!({
            "Condition": condition,
            "Property": property,
            "Value": value,
            "IsRequired": false,
        })
    };
    let codec_profiles: Vec<Value> = DIRECT_VIDEO
        .iter()
        .map(|rule| {
            json!({
                "Type": "Video",
                "Codec": rule.names[0],
                "Conditions": [
                    condition("LessThanEqual", "VideoBitDepth", rule.max_bit_depth.to_string()),
                    condition("LessThanEqual", "Height", MAX_DIRECT_HEIGHT.to_string()),
                    condition("EqualsAny", "VideoRangeType", DIRECT_VIDEO_RANGES.join("|")),
                ],
            })
        })
        .collect();
    json!({
        "Name": "surfcult",
        "MaxStreamingBitrate": MAX_STREAMING_BITRATE,
        "MaxStaticBitrate": MAX_STREAMING_BITRATE,
        "DirectPlayProfiles": [{
            "Type": "Video",
            "VideoCodec": video_codecs.join(","),
            "AudioCodec": DIRECT_AUDIO.join(","),
        }, {
            "Type": "Audio",
            "AudioCodec": DIRECT_AUDIO.join(","),
        }],
        "TranscodingProfiles": [{
            "Type": "Video",
            "Container": TRANSCODE.container,
            "VideoCodec": TRANSCODE.video_codec,
            "AudioCodec": TRANSCODE.audio_codec,
            "Protocol": "http",
            "Context": "Streaming",
        }],
        "CodecProfiles": codec_profiles,
        "ContainerProfiles": [],
        "SubtitleProfiles": subtitle_profiles(),
    })
}

/// Every subtitle format Jellyfin knows, each offered as `External`.
///
/// surfcult never shows subtitles. The profile still has to list them: on
/// Jellyfin 12.4.0 an empty `SubtitleProfiles` makes an item that has any
/// subtitle stream transcode, however well its video and audio fit, and
/// reports every reason (video, audio and subtitle codec) at once. `Drop` and
/// `Embed` were also refused. Checked against the owner's server on
/// 2026-11-24: with this list HEVC and H.264 items direct-play and AV1,
/// H.264 10-bit and 4K HDR items still transcode.
const SUBTITLE_FORMATS: [&str; 14] = [
    "srt", "subrip", "ass", "ssa", "pgssub", "pgs", "dvdsub", "dvbsub", "vtt", "webvtt", "sub",
    "idx", "smi", "microdvd",
];

fn subtitle_profiles() -> Vec<Value> {
    SUBTITLE_FORMATS
        .iter()
        .map(|format| json!({ "Format": format, "Method": "External" }))
        .collect()
}

/// The item id in a transcode path, `/videos/{id}/stream.ts?...`.
fn transcode_item_id(path_and_query: &str) -> &str {
    let path = path_and_query.split('?').next().unwrap_or_default();
    let mut segments = path.trim_start_matches('/').split('/');
    match (segments.next(), segments.next()) {
        (Some(videos), Some(id)) if videos.eq_ignore_ascii_case("videos") => id,
        _ => "",
    }
}

/// Whether two Jellyfin item ids name the same item. Jellyfin writes a GUID
/// with or without dashes and in either case.
fn same_item_id(a: &str, b: &str) -> bool {
    let canonical = |id: &str| -> String {
        id.chars()
            .filter(|c| *c != '-')
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    !a.is_empty() && canonical(a) == canonical(b)
}

/// Jellyfin's `TranscodingUrl` made safe and exact: the key parameters are
/// removed, and the device id, play session, codecs, H.264 bit depth, size,
/// bitrate and start position are set from surfcult's own values whatever Jellyfin put there.
/// The other parameters (media source, audio stream and so on) are kept.
///
/// The error text never repeats the URL, which may hold the key.
fn transcode_url(
    url: &str,
    session: &TranscodeSession,
    start_ticks: u64,
) -> Result<String, &'static str> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if !path.starts_with('/') {
        return Err("the transcode URL is not a path on the server");
    }
    if !path
        .to_ascii_lowercase()
        .ends_with(&format!("/stream.{}", TRANSCODE.container))
    {
        return Err("the transcode is not a progressive MPEG-TS stream");
    }
    let ours = [
        ("DeviceId", encode(&session.device_id)),
        ("PlaySessionId", encode(&session.play_session_id)),
        ("VideoCodec", TRANSCODE.video_codec.to_string()),
        ("h264-videobitdepth", TRANSCODE.video_bit_depth.to_string()),
        ("AudioCodec", TRANSCODE.audio_codec.to_string()),
        ("VideoBitrate", TRANSCODE.video_bitrate.to_string()),
        ("MaxWidth", TRANSCODE.max_width.to_string()),
        ("MaxHeight", TRANSCODE.max_height.to_string()),
        ("StartTimeTicks", start_ticks.to_string()),
    ];
    let replaced = |name: &str| {
        KEY_PARAMS
            .iter()
            .chain(ours.iter().map(|(ours, _)| ours))
            .any(|param| param.eq_ignore_ascii_case(name))
    };
    let mut params: Vec<String> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| !replaced(pair.split_once('=').map_or(*pair, |(name, _)| name)))
        .map(str::to_string)
        .collect();
    params.extend(ours.iter().map(|(name, value)| format!("{name}={value}")));
    Ok(format!("{path}?{}", params.join("&")))
}
