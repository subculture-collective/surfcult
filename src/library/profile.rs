//! What the Pi can play directly, and what Jellyfin must transcode first.
//!
//! This file is the one place the device profile lives. The rules are
//! decisions 3 and 4 of [ADR 0001](../../docs/adr/0001-pi-and-library-findings.md):
//!
//! - Video plays directly when it is H.265 at 8 or 10 bit, or H.264 at 8 bit,
//!   SDR, and at most 1080 pixels tall ([`DIRECT_VIDEO`], [`MAX_DIRECT_HEIGHT`],
//!   [`DIRECT_VIDEO_RANGES`]).
//! - Audio plays directly when it is AAC, MP3, FLAC, Opus, AC3, E-AC3, DTS or
//!   TrueHD ([`DIRECT_AUDIO`]).
//! - Everything else needs a transcode: AV1, H.264 at 10 bit, VVC, VC-1,
//!   MPEG-2, VP9, MPEG-1, anything taller than 1080, HDR or Dolby Vision, and
//!   any other audio codec. Jellyfin transcodes to [`TRANSCODE`]: a progressive
//!   MPEG-TS stream of H.264 and AAC, at most 1080p and about 8 Mbps.
//!
//! [`needs_transcode`] answers for a whole item from the library cache; the
//! tuner uses it to pick a longer tune-in clip. `JellyfinClient::playback`
//! builds the device profile it sends to Jellyfin from the same constants, and
//! Jellyfin then makes the real decision. A change to the ADR edits this file
//! only.
//!
//! [`needs_transcode`] only looks at the item's first video stream and first
//! audio stream, which is what Jellyfin plays by default. Limits of what
//! [`MediaItem`] records:
//!
//! - There is no HDR flag, so HDR and Dolby Vision cannot be detected here.
//!   [`needs_transcode`] treats them as direct play; the profile sent to
//!   Jellyfin does not, so Jellyfin transcodes them.
//! - A missing bit depth is read as 8 bit, because almost all H.264 is 8 bit.
//!   The profile sent to Jellyfin reads it the same way.
//! - A video with no video stream is treated as a transcode, since nothing
//!   shows that it plays directly. An item with no audio stream is fine.
//! - A song has no picture, so only its audio counts.

use super::model::{AudioStream, MediaItem, MediaKind, VideoStream};

/// A video codec the Pi decodes in hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectVideo {
    /// The codec's names in lower case. The first is the one Jellyfin uses;
    /// the others are spellings other tools use for the same codec.
    pub names: &'static [&'static str],
    /// The deepest bit depth that still plays directly.
    pub max_bit_depth: u8,
}

/// The video codecs that play directly, and up to what bit depth.
pub const DIRECT_VIDEO: &[DirectVideo] = &[
    DirectVideo {
        names: &["hevc", "h265"],
        max_bit_depth: 10,
    },
    DirectVideo {
        names: &["h264", "avc"],
        max_bit_depth: 8,
    },
];

/// The tallest picture the Pi plays directly.
pub const MAX_DIRECT_HEIGHT: u32 = 1080;

/// The video ranges that play directly, by Jellyfin's `VideoRangeType` names.
/// HDR10, HLG and Dolby Vision are transcoded.
pub const DIRECT_VIDEO_RANGES: &[&str] = &["SDR"];

/// The audio codecs that play directly, by Jellyfin's names. MPV decodes
/// TrueHD in software for about 0.1 extra cores.
pub const DIRECT_AUDIO: &[&str] = &["aac", "mp3", "flac", "opus", "ac3", "eac3", "dts", "truehd"];

/// What Jellyfin transcodes to when an item does not play directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscodeTarget {
    /// The container, by Jellyfin's name. `ts` asks for a progressive MPEG-TS
    /// stream; HLS took about 19 s to start at an offset and is not used.
    pub container: &'static str,
    pub video_codec: &'static str,
    pub audio_codec: &'static str,
    pub max_width: u32,
    pub max_height: u32,
    /// The video bitrate in bits per second.
    pub video_bitrate: u32,
    /// The bit depth of the transcoded video. The Pi decodes H.264 only at
    /// 8 bit, so a 10-bit H.264 source must be re-encoded, not stream-copied.
    pub video_bit_depth: u8,
}

/// The transcode ADR 0001 measured: 4 to 7 s to start at an offset for 1080p.
pub const TRANSCODE: TranscodeTarget = TranscodeTarget {
    container: "ts",
    video_codec: "h264",
    audio_codec: "aac",
    max_width: 1920,
    max_height: 1080,
    video_bitrate: 8_000_000,
    video_bit_depth: 8,
};

/// Whether Jellyfin must transcode `item` before the Pi can play it.
pub fn needs_transcode(item: &MediaItem) -> bool {
    let video_ok =
        item.kind == MediaKind::Song || item.video.first().is_some_and(video_plays_directly);
    let audio_ok = item.audio.first().is_none_or(audio_plays_directly);
    !(video_ok && audio_ok)
}

/// Whether the Pi decodes this video stream without a transcode.
pub fn video_plays_directly(video: &VideoStream) -> bool {
    if video.height > MAX_DIRECT_HEIGHT {
        return false;
    }
    let codec = video.codec.to_ascii_lowercase();
    let depth = video.bit_depth.unwrap_or(8);
    DIRECT_VIDEO
        .iter()
        .find(|rule| rule.names.contains(&codec.as_str()))
        .is_some_and(|rule| depth <= rule.max_bit_depth)
}

/// Whether MPV plays this audio stream without a transcode.
pub fn audio_plays_directly(audio: &AudioStream) -> bool {
    DIRECT_AUDIO.contains(&audio.codec.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(codec: &str, height: u32, bit_depth: Option<u8>) -> VideoStream {
        VideoStream {
            codec: codec.into(),
            width: height * 16 / 9,
            height,
            bit_depth,
        }
    }

    fn audio(codec: &str) -> AudioStream {
        AudioStream {
            codec: codec.into(),
            channels: 2,
            language: None,
        }
    }

    fn item(video: Option<VideoStream>, audio: Option<AudioStream>) -> MediaItem {
        let mut item = MediaItem::new("x", "X", MediaKind::Movie, 90 * 60_000);
        item.video.extend(video);
        item.audio.extend(audio);
        item
    }

    fn plays(codec: &str, height: u32, depth: Option<u8>) -> bool {
        !needs_transcode(&item(Some(video(codec, height, depth)), Some(audio("aac"))))
    }

    #[test]
    fn h265_plays_directly_at_8_and_10_bit() {
        assert!(plays("hevc", 1080, Some(8)));
        assert!(plays("hevc", 1080, Some(10)));
        assert!(plays("h265", 720, Some(10)));
    }

    #[test]
    fn h264_plays_directly_at_8_bit_only() {
        assert!(plays("h264", 1080, Some(8)));
        assert!(plays("avc", 480, Some(8)));
        assert!(!plays("h264", 1080, Some(10)));
    }

    #[test]
    fn a_missing_bit_depth_is_read_as_8_bit() {
        assert!(plays("h264", 1080, None));
        assert!(plays("hevc", 1080, None));
    }

    #[test]
    fn codecs_the_pi_cannot_decode_need_a_transcode() {
        for codec in [
            "av1",
            "vvc",
            "h266",
            "vc1",
            "mpeg2video",
            "vp9",
            "mpeg1video",
        ] {
            assert!(!plays(codec, 1080, Some(8)), "{codec}");
        }
    }

    #[test]
    fn anything_taller_than_1080_needs_a_transcode() {
        assert!(plays("hevc", 1080, Some(10)));
        assert!(!plays("hevc", 1081, Some(10)));
        assert!(!plays("hevc", 2160, Some(10)));
        assert!(!plays("h264", 1440, Some(8)));
    }

    #[test]
    fn codec_names_are_not_case_sensitive() {
        assert!(plays("HEVC", 1080, Some(10)));
        assert!(!plays("AV1", 1080, Some(8)));
    }

    #[test]
    fn listed_audio_codecs_play_directly() {
        for codec in ["aac", "mp3", "flac", "opus", "ac3", "eac3", "dts", "truehd"] {
            let item = item(Some(video("hevc", 1080, Some(8))), Some(audio(codec)));
            assert!(!needs_transcode(&item), "{codec}");
        }
    }

    #[test]
    fn other_audio_codecs_need_a_transcode() {
        for codec in ["pcm_s16le", "wmapro", "vorbis"] {
            let item = item(Some(video("hevc", 1080, Some(8))), Some(audio(codec)));
            assert!(needs_transcode(&item), "{codec}");
        }
    }

    #[test]
    fn an_item_without_audio_can_still_play_directly() {
        assert!(!needs_transcode(&item(
            Some(video("hevc", 1080, Some(8))),
            None
        )));
    }

    #[test]
    fn an_item_without_a_video_stream_is_a_transcode() {
        assert!(needs_transcode(&item(None, Some(audio("aac")))));
    }

    #[test]
    fn only_the_first_streams_count() {
        let mut item = item(Some(video("hevc", 1080, Some(8))), Some(audio("aac")));
        item.video.push(video("av1", 1080, Some(8)));
        item.audio.push(audio("pcm_s16le"));
        assert!(!needs_transcode(&item));
    }

    #[test]
    fn a_song_counts_only_its_audio() {
        let song = |codec: &str| MediaItem {
            kind: MediaKind::Song,
            ..item(None, Some(audio(codec)))
        };
        assert!(!needs_transcode(&song("flac")));
        assert!(!needs_transcode(&song("mp3")));
        assert!(needs_transcode(&song("alac")));
    }
}
