//! The channel banner as ASS text for MPV's `osd-overlay`.
//!
//! The banner is three lines on a dark translucent box across the bottom of
//! a 1920x1080 frame, inside the 5 % margin a TV may cut off: the channel
//! number and name, the title, and the times. It is set in DejaVu Sans,
//! which the Pi has, in white with a thin black border. MPV scales the
//! layout to the real screen and draws it as OSD, after the display
//! filters, so the CRT shader does not blur it.

use super::card::ass_escape;
use crate::overlay::Banner;

/// The `osd-overlay` id the banner uses. OSD overlay ids are separate from
/// the bitmap layers of `overlay-add`.
pub(super) const BANNER_OSD_ID: u32 = 1;

/// The layout's size; MPV scales it to the screen.
pub(super) const RES: (u32, u32) = (1920, 1080);

/// The box: left, top, right, bottom.
const BOX: (i32, i32, i32, i32) = (96, 760, 1824, 1020);

/// The ASS events of `banner`, one per line, for `osd-overlay` with format
/// `ass-events`. Every text field is escaped, so a name cannot start an
/// override block or an escape.
pub(super) fn banner_ass(banner: &Banner) -> String {
    let (left, top, right, bottom) = BOX;
    let clip = format!("\\clip({left},{top},{right},{bottom})");
    let text = |y: i32, size: u32, bold: bool, colour: &str, text: &str| {
        format!(
            "{{\\an7\\pos({x},{y})\\fnDejaVu Sans\\fs{size}\\b{b}\\bord2\\shad0\\q2{clip}\\1c&H{colour}&\\3c&H000000&}}{text}",
            x = left + 40,
            b = u8::from(bold),
            text = ass_escape(text),
        )
    };
    let lines = [
        format!(
            "{{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H000000&\\1a&H40&\\p1}}m {left} {top} l {right} {top} {right} {bottom} {left} {bottom}{{\\p0}}"
        ),
        text(
            top + 16,
            76,
            true,
            "FFFFFF",
            &format!("{}  {}", banner.number, banner.name),
        ),
        text(top + 110, 58, false, "FFFFFF", &banner.title),
        text(top + 186, 50, false, "D8D8D8", &banner.times),
    ];
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner(name: &str) -> Banner {
        Banner {
            number: 7,
            name: name.into(),
            title: "Show".into(),
            times: "19:00–19:30".into(),
        }
    }

    #[test]
    fn draws_a_box_and_three_lines_of_text() {
        let ass = banner_ass(&banner("CARTOONS"));
        let lines: Vec<&str> = ass.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("\\p1}m 96 760 l 1824 760 1824 1020 96 1020{\\p0}"));
        assert!(lines[1].ends_with("}7  CARTOONS"));
        assert!(lines[1].contains("\\fnDejaVu Sans\\fs76\\b1"));
        assert!(lines[2].ends_with("}Show"));
        assert!(lines[3].ends_with("}19:00–19:30"));
    }

    #[test]
    fn braces_backslashes_and_line_breaks_in_the_text_are_escaped() {
        let ass = banner_ass(&banner("A{\\p1}B\nC"));
        let name = ass.lines().nth(1).unwrap();
        let text = name.split_once("&H000000&}").unwrap().1;
        assert_eq!(text, "7  A\\{\\\u{2060}p1\\}B\\NC");
        assert_eq!(ass.lines().count(), 4);
    }
}
