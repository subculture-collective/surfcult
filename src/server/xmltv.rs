//! `GET /xmltv.xml`: every configured channel and its guide programs for the
//! next [`HORIZON_MS`], as XMLTV.
//!
//! Programs are the guide's cells, not raw segments, so a program cut by
//! breaks is one `<programme>`. The config has no logo field, so channels
//! carry no `<icon>`, and the media model has no description yet, so there
//! is no `<desc>`.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::{ApiError, AppState, read_store};
use crate::guide::{self, Window};
use crate::schedule::HORIZON_MS;

pub(super) async fn guide(State(state): State<AppState>) -> Result<Response, ApiError> {
    let from_ms = (state.now)();
    let window = Window {
        from_ms,
        to_ms: from_ms.saturating_add(HORIZON_MS),
    };
    let numbers: Vec<u32> = state.config.channels.iter().map(|c| c.number).collect();
    let rows = read_store(&state, move |store| {
        guide::build(store, store, &numbers, window)
    })
    .await
    .map_err(|message| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message))?;

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv generator-info-name=\"surfcult\">\n",
    );
    for channel in &state.config.channels {
        xml.push_str(&format!(
            "  <channel id=\"{}\"><display-name>{}</display-name></channel>\n",
            channel_id(channel.number),
            escape(&channel.name),
        ));
    }
    for row in &rows {
        for cell in &row.cells {
            xml.push_str(&format!(
                "  <programme start=\"{}\" stop=\"{}\" channel=\"{}\">\n",
                timestamp(cell.start_ms),
                timestamp(cell.end_ms),
                channel_id(row.channel),
            ));
            // XMLTV requires a title; fall back to the media id.
            let title = cell.title.as_deref().unwrap_or(cell.media_id.as_str());
            xml.push_str(&format!("    <title>{}</title>\n", escape(title)));
            if let Some(description) = &cell.description {
                xml.push_str(&format!("    <desc>{}</desc>\n", escape(description)));
            }
            for genre in &cell.genres {
                xml.push_str(&format!("    <category>{}</category>\n", escape(genre)));
            }
            xml.push_str("  </programme>\n");
        }
    }
    xml.push_str("</tv>\n");
    Ok(([(header::CONTENT_TYPE, "application/xml")], xml).into_response())
}

fn channel_id(number: u32) -> String {
    format!("{number}.surfcult")
}

/// An XMLTV time: `YYYYMMDDHHMMSS +0000`.
fn timestamp(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02} +0000",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab, newline and CR are not XML.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}
