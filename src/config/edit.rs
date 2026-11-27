//! One channel's settings as `channels.toml` writes them, read from and
//! written back into the file's text for the web admin.
//!
//! The text is edited in place with `toml_edit`, so comments, blank lines,
//! key order and every other table stay as they were. Only the keys of
//! [`ChannelSettings`] in the one `[[channel]]` table change. The result is
//! not checked here: the caller parses it with [`Config::parse`](super::Config::parse)
//! before writing it.

use std::fmt;

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// The channel keys the web admin edits, as strings as they are written in
/// the file. `channel.number` is not one of them: it is the channel's
/// identity in the schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSettings {
    pub name: String,
    /// The one source of a `sequential` or `shuffle` channel. `None` for a
    /// `blocks` or `weighted` channel, whose sources are edited in the file.
    pub source: Option<SourceSetting>,
    /// `sequential` or `shuffle`, with `source`; `None` without it.
    pub mode: Option<String>,
    /// A duration such as `30m`; `None` for no grid.
    pub grid: Option<String>,
    pub breaks: Option<BreaksSetting>,
    /// An interstitial pool name; `None` for no filler.
    pub filler: Option<String>,
}

/// `source = { <kind> = "<name>" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSetting {
    /// One of [`SOURCE_KINDS`].
    pub kind: String,
    pub name: String,
}

/// `breaks = { every, length, pool, pad }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreaksSetting {
    pub every: String,
    pub length: String,
    pub pool: String,
    #[serde(default)]
    pub pad: Option<String>,
}

/// The keys a `source` table can have, as in [`Source`](super::Source).
pub const SOURCE_KINDS: &[&str] = &[
    "jellyfin_library",
    "jellyfin_collection",
    "jellyfin_tag",
    "jellyfin_genre",
    "jellyfin_series",
];

/// The modes of a channel with one source.
const ONE_SOURCE_MODES: &[&str] = &["sequential", "shuffle"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The text has no `[[channel]]` with this number.
    NoChannel(u32),
    /// The text is not TOML.
    Unreadable(String),
    /// The settings cannot be written as asked.
    Refused(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoChannel(number) => write!(f, "channels.toml has no channel {number}"),
            Self::Unreadable(why) => write!(f, "channels.toml is not TOML: {why}"),
            Self::Refused(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for EditError {}

/// The settings of channel `number` in `text`.
pub fn channel_settings(text: &str, number: u32) -> Result<ChannelSettings, EditError> {
    let mut doc = parse(text)?;
    Ok(read(channel(&mut doc, number)?))
}

/// `text` with channel `number`'s keys set to `settings`. An empty string
/// counts as left out, which removes an optional key. The source and mode
/// of a `blocks` or `weighted` channel cannot be set, and those of a channel
/// with one source cannot be removed.
pub fn set_channel(
    text: &str,
    number: u32,
    settings: &ChannelSettings,
) -> Result<String, EditError> {
    let mut doc = parse(text)?;
    let table = channel(&mut doc, number)?;
    let one_source = read(table).source.is_some();

    let name = settings.name.trim();
    if name.is_empty() {
        return Err(refused("the name must not be empty"));
    }
    let source = settings
        .source
        .as_ref()
        .map(|s| (s.kind.trim(), s.name.trim()));
    let mode = given(&settings.mode);
    match (one_source, source, mode) {
        (true, Some((kind, name)), Some(mode)) => {
            if !SOURCE_KINDS.contains(&kind) {
                return Err(refused(format!(
                    "the source kind must be one of {}, not {kind:?}",
                    SOURCE_KINDS.join(", ")
                )));
            }
            if name.is_empty() {
                return Err(refused("the source name must not be empty"));
            }
            if !ONE_SOURCE_MODES.contains(&mode) {
                return Err(refused(format!(
                    "the mode must be sequential or shuffle, not {mode:?}"
                )));
            }
            let mut inline = InlineTable::new();
            inline.insert(kind, Value::from(name));
            replace(table, "source", Value::InlineTable(inline));
            set_str(table, "mode", Some(mode));
        }
        (true, _, _) => {
            return Err(refused(
                "a channel with one source needs a source and a mode",
            ));
        }
        (false, None, None) => {}
        (false, _, _) => {
            return Err(refused(format!(
                "channel {number} plays blocks or weighted sources; change its sources in channels.toml"
            )));
        }
    }
    set_str(table, "name", Some(name));
    set_str(table, "grid", given(&settings.grid));
    set_str(table, "filler", given(&settings.filler));
    match &settings.breaks {
        None => {
            table.remove("breaks");
        }
        Some(breaks) => {
            let fields = [
                ("every", Some(breaks.every.trim())),
                ("length", Some(breaks.length.trim())),
                ("pool", Some(breaks.pool.trim())),
                ("pad", given(&breaks.pad)),
            ];
            match table.get_mut("breaks").and_then(Item::as_table_like_mut) {
                // A `breaks` table already there keeps its form and comments.
                Some(existing) => {
                    let keys = existing.len();
                    for (key, value) in fields {
                        set_str(existing, key, value);
                    }
                    // An inline table that gained or lost `pad` is spaced
                    // afresh, or the old last value's spacing stays behind.
                    if existing.len() != keys
                        && let Some(inline) =
                            table.get_mut("breaks").and_then(Item::as_inline_table_mut)
                    {
                        inline.fmt();
                    }
                }
                None => {
                    let mut inline = InlineTable::new();
                    for (key, value) in fields {
                        set_str(&mut inline, key, value);
                    }
                    table.insert("breaks", Item::Value(Value::InlineTable(inline)));
                }
            }
        }
    }
    Ok(doc.to_string())
}

fn parse(text: &str) -> Result<DocumentMut, EditError> {
    text.parse::<DocumentMut>()
        .map_err(|e| EditError::Unreadable(e.to_string()))
}

fn channel(doc: &mut DocumentMut, number: u32) -> Result<&mut Table, EditError> {
    doc.get_mut("channel")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|channels| {
            channels.iter_mut().find(|table| {
                table.get("number").and_then(Item::as_integer) == Some(i64::from(number))
            })
        })
        .ok_or(EditError::NoChannel(number))
}

fn read(table: &Table) -> ChannelSettings {
    let text = |table: &dyn TableLike, key: &str| {
        table.get(key).and_then(Item::as_str).map(str::to_string)
    };
    let mode = text(table, "mode");
    // One source: a `source` table with one known key and a one-source mode.
    let source = table
        .get("source")
        .and_then(Item::as_table_like)
        .filter(|source| source.len() == 1)
        .and_then(|source| source.iter().next())
        .and_then(|(kind, name)| {
            Some(SourceSetting {
                kind: kind.to_string(),
                name: name.as_str()?.to_string(),
            })
        })
        .filter(|source| SOURCE_KINDS.contains(&source.kind.as_str()))
        .filter(|_| {
            mode.as_deref()
                .is_some_and(|m| ONE_SOURCE_MODES.contains(&m))
        });
    let breaks = table
        .get("breaks")
        .and_then(Item::as_table_like)
        .map(|breaks| BreaksSetting {
            every: text(breaks, "every").unwrap_or_default(),
            length: text(breaks, "length").unwrap_or_default(),
            pool: text(breaks, "pool").unwrap_or_default(),
            pad: text(breaks, "pad"),
        });
    ChannelSettings {
        name: text(table, "name").unwrap_or_default(),
        mode: source.as_ref().and(mode),
        source,
        grid: text(table, "grid"),
        breaks,
        filler: text(table, "filler"),
    }
}

/// The trimmed text, or `None` when it is left out or empty.
fn given(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// Sets `key` to the string `value`, or removes it for `None`.
fn set_str(table: &mut dyn TableLike, key: &str, value: Option<&str>) {
    match value {
        Some(value) => replace(table, key, Value::from(value)),
        None => {
            table.remove(key);
        }
    }
}

/// Sets `key` to `new`, keeping the spacing and comment around a value
/// already there.
fn replace(table: &mut dyn TableLike, key: &str, mut new: Value) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
}

fn refused(why: impl Into<String>) -> EditError {
    EditError::Refused(why.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const TEXT: &str = r#"timezone = "America/Chicago"

# Cartoons all day.
[[channel]]
number = 3
name = "SATURDAY CARTOONS"
source = { jellyfin_collection = "Cartoons" }
mode = "shuffle"
grid = "30m"                                          # programs start on the half hour
breaks = { every = "11m", length = "2m", pool = "ads" }
filler = "filler"                                     # pool used to fill grid gaps

[[channel]]
number = 4
name = "MOVIE NIGHT"
source = { jellyfin_library = "Movies" }
mode = "sequential"

[[channel]]
number = 5
name = "WEIGHTED"
sources = [{ jellyfin_genre = "Comedy", weight = 3 }, { jellyfin_tag = "Shorts" }]
mode = "weighted"

[[interstitial_pool]]
name = "ads"
path = "/clips/ads"

[[interstitial_pool]]
name = "filler"
path = "/clips/filler"
"#;

    fn settings(number: u32) -> ChannelSettings {
        channel_settings(TEXT, number).unwrap()
    }

    #[test]
    fn reads_a_channel_as_written() {
        assert_eq!(
            settings(3),
            ChannelSettings {
                name: "SATURDAY CARTOONS".into(),
                source: Some(SourceSetting {
                    kind: "jellyfin_collection".into(),
                    name: "Cartoons".into()
                }),
                mode: Some("shuffle".into()),
                grid: Some("30m".into()),
                breaks: Some(BreaksSetting {
                    every: "11m".into(),
                    length: "2m".into(),
                    pool: "ads".into(),
                    pad: None
                }),
                filler: Some("filler".into()),
            }
        );
        let weighted = settings(5);
        assert_eq!((weighted.source, weighted.mode), (None, None));
        assert_eq!(channel_settings(TEXT, 9), Err(EditError::NoChannel(9)));
    }

    #[test]
    fn unchanged_settings_leave_the_text_as_it_was() {
        for number in [3, 4, 5] {
            assert_eq!(set_channel(TEXT, number, &settings(number)).unwrap(), TEXT);
        }
    }

    #[test]
    fn a_change_keeps_comments_and_the_other_channels() {
        let mut new = settings(3);
        new.name = "  CARTOON PARTY ".into();
        new.grid = Some("1h".into());
        new.source = Some(SourceSetting {
            kind: "jellyfin_series".into(),
            name: "Looney".into(),
        });
        new.mode = Some("sequential".into());
        new.breaks.as_mut().unwrap().pad = Some("filler".into());
        let text = set_channel(TEXT, 3, &new).unwrap();
        let expected = TEXT
            .replace("\"SATURDAY CARTOONS\"", "\"CARTOON PARTY\"")
            .replace(
                "{ jellyfin_collection = \"Cartoons\" }\nmode = \"shuffle\"",
                "{ jellyfin_series = \"Looney\" }\nmode = \"sequential\"",
            )
            .replace("grid = \"30m\"", "grid = \"1h\"")
            .replace("pool = \"ads\" }", "pool = \"ads\", pad = \"filler\" }");
        assert_eq!(text, expected);
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.channels[0].name, "CARTOON PARTY");
        assert_eq!(config.channels[1], Config::parse(TEXT).unwrap().channels[1]);
    }

    #[test]
    fn empty_values_remove_keys_and_new_ones_are_added() {
        let mut cleared = settings(3);
        cleared.grid = Some(" ".into());
        cleared.breaks = None;
        cleared.filler = None;
        let text = set_channel(TEXT, 3, &cleared).unwrap();
        let config = Config::parse(&text).unwrap();
        let channel = &config.channels[0];
        assert_eq!(
            (channel.grid, &channel.breaks, &channel.filler),
            (None, &None, &None)
        );
        assert!(!text.contains("pool used to fill grid gaps"));
        assert!(text.contains("# Cartoons all day."));

        let mut added = settings(4);
        added.grid = Some("30m".into());
        added.breaks = Some(BreaksSetting {
            every: "15m".into(),
            length: "3m".into(),
            pool: "ads".into(),
            pad: Some(String::new()),
        });
        added.filler = Some("filler".into());
        let text = set_channel(TEXT, 4, &added).unwrap();
        let channel = &Config::parse(&text).unwrap().channels[1];
        assert_eq!(channel.grid, Some(std::time::Duration::from_secs(1800)));
        let breaks = channel.breaks.as_ref().unwrap();
        assert_eq!((breaks.pool.as_str(), &breaks.pad), ("ads", &None));
        assert_eq!(channel.filler.as_deref(), Some("filler"));
    }

    #[test]
    fn a_breaks_table_keeps_its_form() {
        let text = TEXT.replace(
            "breaks = { every = \"11m\", length = \"2m\", pool = \"ads\" }\n",
            "",
        ) + "";
        let text = text.replace(
            "filler = \"filler\"                                     # pool used to fill grid gaps\n",
            "filler = \"filler\"\n\n[channel.breaks]\nevery = \"11m\" # often\nlength = \"2m\"\npool = \"ads\"\n",
        );
        let mut new = channel_settings(&text, 3).unwrap();
        new.breaks.as_mut().unwrap().every = "12m".into();
        let written = set_channel(&text, 3, &new).unwrap();
        assert_eq!(
            written,
            text.replace("every = \"11m\" # often", "every = \"12m\" # often")
        );
    }

    #[test]
    fn refuses_what_it_cannot_write() {
        let refused = |number, change: fn(&mut ChannelSettings)| {
            let mut new = settings(number);
            change(&mut new);
            matches!(set_channel(TEXT, number, &new), Err(EditError::Refused(_)))
        };
        assert!(refused(3, |s| s.name = " ".into()));
        assert!(refused(3, |s| s.source = None));
        assert!(refused(3, |s| s.mode = Some("weighted".into())));
        assert!(refused(3, |s| s.source.as_mut().unwrap().kind = "path".into()));
        assert!(refused(3, |s| s.source.as_mut().unwrap().name = String::new()));
        assert!(refused(5, |s| s.mode = Some("shuffle".into())));
        assert_eq!(
            set_channel(TEXT, 9, &settings(3)),
            Err(EditError::NoChannel(9))
        );
        assert!(matches!(
            set_channel("not = [toml", 3, &settings(3)),
            Err(EditError::Unreadable(_))
        ));
    }
}
