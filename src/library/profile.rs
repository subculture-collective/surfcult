//! What the Pi can play directly, and what Jellyfin must transcode first.
//!
//! The rules are decision 3 of [ADR 0001](../../docs/adr/0001-pi-and-library-findings.md):
//!
//! - Video plays directly when it is H.265 at 8 or 10 bit, or H.264 at 8 bit,
//!   and is at most 1080 pixels tall.
//! - Audio plays directly when it is AAC, MP3, FLAC, Opus, AC3, E-AC3, DTS or
//!   TrueHD.
//! - Everything else needs a transcode: AV1, H.264 at 10 bit, VVC, VC-1,
//!   MPEG-2, VP9, MPEG-1, anything taller than 1080, and any other audio codec.
//!
//! [`needs_transcode`] answers for a whole item. It only looks at the item's
//! first video stream and first audio stream, which is what Jellyfin plays by
//! default. The tuner uses it to pick a longer tune-in clip; playback URLs
//! will use it later.
//!
//! Limits of what [`MediaItem`] records:
//!
//! - There is no HDR flag, so HDR and Dolby Vision cannot be detected here. ADR
//!   0001 counts them as transcodes; this function treats them as direct play
//!   until the model gains the flag.
//! - A missing bit depth is read as 8 bit, because almost all H.264 is 8 bit.
//! - An item with no video stream is treated as a transcode, since nothing
//!   shows that it plays directly. An item with no audio stream is fine.

use super::model::{AudioStream, MediaItem, VideoStream};

/// The tallest picture the Pi plays directly.
pub const MAX_DIRECT_HEIGHT: u32 = 1080;

/// Whether Jellyfin must transcode `item` before the Pi can play it.
pub fn needs_transcode(item: &MediaItem) -> bool {
    let video_ok = item.video.first().is_some_and(video_plays_directly);
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
    match codec.as_str() {
        "hevc" | "h265" => depth <= 10,
        "h264" | "avc" => depth <= 8,
        _ => false,
    }
}

/// Whether MPV plays this audio stream without a transcode.
pub fn audio_plays_directly(audio: &AudioStream) -> bool {
    matches!(
        audio.codec.to_ascii_lowercase().as_str(),
        "aac" | "mp3" | "flac" | "opus" | "ac3" | "eac3" | "dts" | "truehd"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::MediaKind;

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
}
