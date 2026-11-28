//! The file as written, and the checks that turn it into [`Config`].
//!
//! Values that can be wrong in ways worth naming (modes, durations, times, the
//! time zone) stay strings here so each problem is reported against its key.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::input::KeyMap;

use super::duration::{parse_duration, parse_minute_of_day};
use super::{
    Between, Block, Breaks, Channel, ClipKind, Config, DEFAULT_TUNE_IN_POOL, Day, Display, Filter,
    Interlude, InterstitialPool, Lineup, Mode, Plan, PoolUse, Problem, Rotation, SignOff, Source,
    Tz, WeightedPool, WeightedSource,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    timezone: String,
    #[serde(default)]
    channel: Vec<RawChannel>,
    #[serde(default)]
    interstitial_pool: Vec<RawPool>,
    display: Option<RawDisplay>,
    tune_in_pool: Option<String>,
    keys: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDisplay {
    #[serde(default)]
    filters: Vec<String>,
    standby_filters: Option<Vec<String>>,
    glitch_every: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChannel {
    number: u32,
    name: String,
    source: Option<Source>,
    sources: Option<Vec<RawWeightedSource>>,
    mode: String,
    grid: Option<String>,
    breaks: Option<RawBreaks>,
    between: Option<RawBetween>,
    filler: Option<String>,
    default: Option<RawLineup>,
    #[serde(default)]
    block: Vec<RawBlock>,
    sign_off: Option<RawSignOff>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSignOff {
    at: String,
    pool: String,
    clip: Option<String>,
    until: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBreaks {
    every: String,
    length: String,
    pool: Option<String>,
    pools: Option<Vec<RawPoolEntry>>,
    open: Option<String>,
    close: Option<String>,
    pad: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBetween {
    length: Option<String>,
    pool: Option<String>,
    pools: Option<Vec<RawPoolEntry>>,
    open: Option<String>,
    close: Option<String>,
}

/// The keys `breaks` and `between` share, as [`RawInterlude`].
macro_rules! interlude_keys {
    ($raw:ty) => {
        impl $raw {
            fn fill(&self) -> RawInterlude<'_> {
                RawInterlude {
                    pool: self.pool.as_deref(),
                    pools: self.pools.as_deref(),
                    open: self.open.as_deref(),
                    close: self.close.as_deref(),
                }
            }
        }
    };
}
interlude_keys!(RawBreaks);
interlude_keys!(RawBetween);

/// The pool keys of a `breaks` or `between` table.
#[derive(Clone, Copy)]
struct RawInterlude<'a> {
    pool: Option<&'a str>,
    pools: Option<&'a [RawPoolEntry]>,
    open: Option<&'a str>,
    close: Option<&'a str>,
}

impl<'a> RawInterlude<'a> {
    /// The pools it draws from by weight, whether named by `pool` or listed
    /// in `pools`.
    fn drawn(self) -> impl Iterator<Item = &'a str> {
        self.pool.into_iter().chain(
            self.pools
                .into_iter()
                .flatten()
                .map(|entry| entry.pool.as_str()),
        )
    }

    /// Every pool it names.
    fn named(self) -> impl Iterator<Item = &'a str> {
        self.drawn().chain(self.open).chain(self.close)
    }
}

/// One entry of `pools`: `{ pool = "ads", weight = 3 }`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPoolEntry {
    pool: String,
    weight: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    #[serde(default)]
    days: Vec<String>,
    start: String,
    end: String,
    source: Option<Source>,
    sources: Option<Vec<RawWeightedSource>>,
    mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLineup {
    source: Option<Source>,
    sources: Option<Vec<RawWeightedSource>>,
    mode: String,
}

/// One entry of `sources`: a source key with its value, as in `source`, and an
/// optional `weight`, for example `{ jellyfin_series = "Name", weight = 2 }`.
/// Read by hand so that a key that is neither is a parse error with its line.
struct RawWeightedSource {
    source: Source,
    weight: Option<i64>,
}

impl<'de> Deserialize<'de> for RawWeightedSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Keeps in step with [`Source`]'s variants, in snake case.
        const KEYS: &[&str] = &[
            "jellyfin_library",
            "jellyfin_collection",
            "jellyfin_tag",
            "jellyfin_genre",
            "jellyfin_series",
            "weight",
        ];

        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = RawWeightedSource;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a source with an optional weight, such as { jellyfin_series = \"Name\", weight = 2 }")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut source: Option<Source> = None;
                let mut weight: Option<i64> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "weight" {
                        if weight.is_some() {
                            return Err(de::Error::duplicate_field("weight"));
                        }
                        weight = Some(map.next_value()?);
                        continue;
                    }
                    let make: fn(String) -> Source = match key.as_str() {
                        "jellyfin_library" => Source::JellyfinLibrary,
                        "jellyfin_collection" => Source::JellyfinCollection,
                        "jellyfin_tag" => Source::JellyfinTag,
                        "jellyfin_genre" => Source::JellyfinGenre,
                        "jellyfin_series" => Source::JellyfinSeries,
                        _ => return Err(de::Error::unknown_field(&key, KEYS)),
                    };
                    let parsed = make(map.next_value()?);
                    if source.replace(parsed).is_some() {
                        return Err(de::Error::custom(
                            "an entry names one source; use one entry per source",
                        ));
                    }
                }
                let source = source.ok_or_else(|| {
                    de::Error::custom("an entry needs a source, such as jellyfin_series = \"Name\"")
                })?;
                Ok(RawWeightedSource { source, weight })
            }
        }

        deserializer.deserialize_map(EntryVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPool {
    name: String,
    path: PathBuf,
    kind: Option<String>,
    rotation: Option<String>,
    repeat_after: Option<String>,
}

#[derive(Default)]
struct Problems(Vec<Problem>);

impl Problems {
    fn add(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(Problem {
            field: field.into(),
            message: message.into(),
        });
    }
}

impl RawConfig {
    pub fn into_config(self) -> Result<Config, Vec<Problem>> {
        let mut problems = Problems::default();

        let timezone = match self.timezone.parse::<Tz>() {
            Ok(tz) => Some(tz),
            Err(_) => {
                problems.add(
                    "timezone",
                    format!(
                        "unknown time zone {:?}; use an IANA name such as America/Chicago",
                        self.timezone
                    ),
                );
                None
            }
        };

        // A pool a channel's breaks draw from holds commercials unless its
        // `kind` says otherwise; any other pool holds filler.
        let fills_breaks = |name: &str| {
            self.channel.iter().any(|raw| {
                raw.breaks
                    .as_ref()
                    .is_some_and(|b| b.fill().drawn().any(|pool| pool == name))
            })
        };
        let mut pools = Vec::new();
        let mut pool_index: HashMap<&str, usize> = HashMap::new();
        for (i, pool) in self.interstitial_pool.iter().enumerate() {
            let at = |key: &str| format!("interstitial_pool[{i}].{key}");
            let kind = match pool.kind.as_deref() {
                None if fills_breaks(&pool.name) => ClipKind::Commercial,
                None => ClipKind::Filler,
                Some(name) => ClipKind::from_name(name).unwrap_or_else(|| {
                    let known: Vec<&str> = ClipKind::ALL.iter().map(|k| k.name()).collect();
                    problems.add(
                        at("kind"),
                        format!("unknown kind {name:?}; use {}", known.join(", ")),
                    );
                    ClipKind::Filler
                }),
            };
            let rotation = match pool.rotation.as_deref() {
                None | Some("order") => Rotation::Order,
                Some("shuffle") => Rotation::Shuffle,
                Some(other) => {
                    problems.add(
                        at("rotation"),
                        format!("unknown rotation {other:?}; use order or shuffle"),
                    );
                    Rotation::Order
                }
            };
            let repeat_after = pool
                .repeat_after
                .as_deref()
                .and_then(|text| positive_duration(text, &at("repeat_after"), &mut problems));
            if pool.name.trim().is_empty() {
                problems.add(format!("interstitial_pool[{i}].name"), "must not be empty");
            }
            if let Some(first) = pool_index.insert(pool.name.as_str(), i) {
                problems.add(
                    format!("interstitial_pool[{i}].name"),
                    format!(
                        "{:?} is already used by interstitial_pool[{first}]",
                        pool.name
                    ),
                );
            }
            pools.push(InterstitialPool {
                name: pool.name.clone(),
                path: pool.path.clone(),
                kind,
                rotation,
                repeat_after,
            });
        }

        let mut channels = Vec::new();
        let mut numbers: HashMap<u32, usize> = HashMap::new();
        for (i, raw) in self.channel.iter().enumerate() {
            if let Some(first) = numbers.insert(raw.number, i) {
                problems.add(
                    format!("channel[{i}].number"),
                    format!("{} is already used by channel[{first}]", raw.number),
                );
            }
            if let Some(channel) = raw.check(i, &pool_index, &pools, &mut problems) {
                channels.push(channel);
            }
        }

        // Rotation and repeat limits only drive the drawing of break and
        // `between` clips, so they are refused on a pool no channel draws
        // such clips from rather than silently ignored.
        for (i, pool) in self.interstitial_pool.iter().enumerate() {
            let rotates = self.channel.iter().any(|raw| {
                let breaks = raw.breaks.as_ref().map(RawBreaks::fill);
                let between = raw.between.as_ref().map(RawBetween::fill);
                breaks
                    .into_iter()
                    .chain(between)
                    .any(|fill| fill.named().any(|name| name == pool.name))
            });
            if rotates {
                continue;
            }
            for (key, set) in [
                ("rotation", pool.rotation.is_some()),
                ("repeat_after", pool.repeat_after.is_some()),
            ] {
                if set {
                    problems.add(
                        format!("interstitial_pool[{i}].{key}"),
                        format!(
                            "applies only to a pool that breaks or between draw from, and no channel draws from {:?}",
                            pool.name
                        ),
                    );
                }
            }
        }

        if let Some(name) = &self.tune_in_pool
            && !pool_index.contains_key(name.as_str())
        {
            problems.add(
                "tune_in_pool",
                format!("no interstitial_pool named {name:?}"),
            );
        }
        let tune_in_pool = self
            .tune_in_pool
            .clone()
            .unwrap_or_else(|| DEFAULT_TUNE_IN_POOL.to_string());

        let display = self
            .display
            .as_ref()
            .map_or_else(Display::default, |raw| raw.check(&mut problems));

        let keys = match self.keys.as_ref().map(KeyMap::from_table) {
            None => KeyMap::default(),
            Some(Ok(keys)) => keys,
            Some(Err(found)) => {
                for (field, message) in found {
                    problems.add(field, message);
                }
                KeyMap::default()
            }
        };

        match (timezone, problems.0.is_empty()) {
            (Some(timezone), true) => Ok(Config {
                timezone,
                channels,
                pools,
                display,
                tune_in_pool,
                keys,
            }),
            _ => Err(problems.0),
        }
    }
}

impl RawChannel {
    fn check(
        &self,
        i: usize,
        pools: &HashMap<&str, usize>,
        configured: &[InterstitialPool],
        problems: &mut Problems,
    ) -> Option<Channel> {
        let at = |key: &str| format!("channel[{i}].{key}");
        let before = problems.0.len();

        if self.name.trim().is_empty() {
            problems.add(at("name"), "must not be empty");
        }

        let plan = self.plan(i, problems);

        let grid = self
            .grid
            .as_deref()
            .and_then(|text| positive_duration(text, &at("grid"), problems));

        let breaks = self.breaks.as_ref().and_then(|b| {
            let every = positive_duration(&b.every, &at("breaks.every"), problems);
            let length = positive_duration(&b.length, &at("breaks.length"), problems);
            let fill = check_interlude(b.fill(), &at("breaks"), pools, configured, problems);
            if let Some(pad) = &b.pad
                && !pools.contains_key(pad.as_str())
            {
                problems.add(
                    at("breaks.pad"),
                    format!("no interstitial_pool named {pad:?}"),
                );
            }
            match (every, length) {
                (Some(every), Some(length)) if length >= every => {
                    problems.add(at("breaks.length"), "must be shorter than breaks.every");
                    None
                }
                (Some(every), Some(length)) => Some(Breaks {
                    every,
                    length,
                    fill: fill?,
                    pad: b.pad.clone(),
                }),
                _ => None,
            }
        });

        let between = self.between.as_ref().and_then(|b| {
            let length = b
                .length
                .as_deref()
                .map(|text| positive_duration(text, &at("between.length"), problems));
            let fill = check_interlude(b.fill(), &at("between"), pools, configured, problems)?;
            Some(Between {
                length: match length {
                    Some(checked) => Some(checked?),
                    None => None,
                },
                fill,
            })
        });

        if let Some(filler) = &self.filler
            && !pools.contains_key(filler.as_str())
        {
            problems.add(
                at("filler"),
                format!("no interstitial_pool named {filler:?}"),
            );
        }

        let sign_off = self
            .sign_off
            .as_ref()
            .and_then(|raw| check_sign_off(raw, i, pools, problems));

        if problems.0.len() > before {
            return None;
        }
        Some(Channel {
            number: self.number,
            name: self.name.clone(),
            plan: plan?,
            grid,
            breaks,
            between,
            filler: self.filler.clone(),
            sign_off,
        })
    }

    fn plan(&self, i: usize, problems: &mut Problems) -> Option<Plan> {
        let at = |key: &str| format!("channel[{i}].{key}");
        if self.mode == "blocks" {
            if self.source.is_some() {
                problems.add(
                    at("source"),
                    "not allowed with mode \"blocks\"; set a source on each block",
                );
            }
            if self.block.is_empty() {
                problems.add(
                    at("mode"),
                    "mode \"blocks\" needs at least one [[channel.block]]",
                );
                return None;
            }
            if self.sources.is_some() {
                problems.add(
                    at("sources"),
                    "not allowed with mode \"blocks\"; set sources on each block",
                );
            }
            let default = self.default.as_ref().and_then(|raw| {
                let prefix = format!("channel[{i}].default");
                let mode = block_mode(&raw.mode, &format!("{prefix}.mode"), problems);
                let sources = check_lineup(
                    &prefix,
                    raw.source.as_ref(),
                    raw.sources.as_deref(),
                    mode,
                    problems,
                );
                Some(Lineup {
                    sources,
                    mode: mode?,
                })
            });
            let blocks = check_blocks(&self.block, i, problems)?;
            return Some(Plan::Blocks { blocks, default });
        }

        if !self.block.is_empty() {
            problems.add(at("block"), "blocks need mode = \"blocks\"");
        }
        if self.default.is_some() {
            problems.add(at("default"), "a default needs mode = \"blocks\"");
        }
        let mode = parse_mode(&self.mode, &at("mode"), problems);
        let sources = check_lineup(
            &format!("channel[{i}]"),
            self.source.as_ref(),
            self.sources.as_deref(),
            mode,
            problems,
        );
        Some(Plan::Fixed {
            sources,
            mode: mode?,
        })
    }
}

/// Checks the pools of a `breaks` or `between` table at `prefix`: exactly
/// one of `pool` and `pools`, every name an `[[interstitial_pool]]`, each
/// listed once with a weight of at least 1. `None` when something is wrong.
fn check_interlude(
    raw: RawInterlude<'_>,
    prefix: &str,
    pools: &HashMap<&str, usize>,
    configured: &[InterstitialPool],
    problems: &mut Problems,
) -> Option<Interlude> {
    let before = problems.0.len();
    let field = |key: &str| format!("{prefix}.{key}");
    let pool_use = |name: &str, key: &str, problems: &mut Problems| match pools.get(name) {
        Some(&j) => {
            let pool = &configured[j];
            Some(PoolUse {
                name: pool.name.clone(),
                kind: pool.kind,
                rotation: pool.rotation,
                repeat_after: pool.repeat_after,
            })
        }
        None => {
            problems.add(key, format!("no interstitial_pool named {name:?}"));
            None
        }
    };
    let mut drawn = Vec::new();
    match (raw.pool, raw.pools) {
        (Some(name), None) => {
            if let Some(pool) = pool_use(name, &field("pool"), problems) {
                drawn.push(WeightedPool { pool, weight: 1 });
            }
        }
        (None, Some(list)) if !list.is_empty() => {
            for (j, entry) in list.iter().enumerate() {
                let at = format!("{prefix}.pools[{j}]");
                if let Some(first) = list[..j].iter().position(|other| other.pool == entry.pool) {
                    problems.add(
                        &at,
                        format!(
                            "is the same pool as {prefix}.pools[{first}]; list it once and add the weights"
                        ),
                    );
                }
                let weight = match entry.weight {
                    None => Some(1),
                    Some(weight) => u32::try_from(weight).ok().filter(|w| *w >= 1).or_else(|| {
                        problems.add(
                            format!("{at}.weight"),
                            format!("{weight} is not a weight; use a whole number of at least 1"),
                        );
                        None
                    }),
                };
                let pool = pool_use(&entry.pool, &format!("{at}.pool"), problems);
                if let (Some(pool), Some(weight)) = (pool, weight) {
                    drawn.push(WeightedPool { pool, weight });
                }
            }
        }
        (Some(_), Some(_)) => problems.add(
            field("pools"),
            "set pool or pools, not both; pool = \"ads\" is pools = [{ pool = \"ads\" }]",
        ),
        (None, _) => problems.add(
            field("pools"),
            "missing; name the clips to draw, such as pools = [{ pool = \"ads\", weight = 3 }] or pool = \"ads\"",
        ),
    }
    let open = raw
        .open
        .map(|name| pool_use(name, &field("open"), problems));
    let close = raw
        .close
        .map(|name| pool_use(name, &field("close"), problems));
    if problems.0.len() > before {
        return None;
    }
    Some(Interlude {
        pools: drawn,
        open: open.flatten(),
        close: close.flatten(),
    })
}

fn check_sign_off(
    raw: &RawSignOff,
    channel: usize,
    pools: &HashMap<&str, usize>,
    problems: &mut Problems,
) -> Option<SignOff> {
    let at = |key: &str| format!("channel[{channel}].sign_off.{key}");
    let time = |text: &str, key: &str, problems: &mut Problems| match parse_minute_of_day(text) {
        Ok(minute) if minute < 24 * 60 => Some(minute),
        Ok(_) => {
            problems.add(at(key), "use 00:00 for midnight");
            None
        }
        Err(message) => {
            problems.add(at(key), message);
            None
        }
    };
    let at_minute = time(&raw.at, "at", problems);
    let until_minute = time(&raw.until, "until", problems);
    if at_minute.is_some() && at_minute == until_minute {
        problems.add(at("until"), "must differ from at");
    }
    if !pools.contains_key(raw.pool.as_str()) {
        problems.add(
            at("pool"),
            format!("no interstitial_pool named {:?}", raw.pool),
        );
    }
    if let Some(clip) = &raw.clip
        && (clip.is_empty() || clip == "." || clip == ".." || clip.contains(['/', '\\']))
    {
        problems.add(
            at("clip"),
            format!("{clip:?} is not a file name; name one file in the pool, with no path"),
        );
    }
    Some(SignOff {
        at_minute: at_minute?,
        until_minute: until_minute?,
        pool: raw.pool.clone(),
        clip: raw.clip.clone(),
    })
}

impl RawDisplay {
    fn check(&self, problems: &mut Problems) -> Display {
        let defaults = Display::default();
        let filters = filters_named(&self.filters, "display.filters", problems);
        let standby_filters = match &self.standby_filters {
            Some(names) => filters_named(names, "display.standby_filters", problems),
            None => defaults.standby_filters,
        };
        let glitch_every = match &self.glitch_every {
            Some(text) => positive_duration(text, "display.glitch_every", problems)
                .unwrap_or(defaults.glitch_every),
            None => defaults.glitch_every,
        };
        Display {
            filters,
            standby_filters,
            glitch_every,
        }
    }
}

/// The filters `names` stands for. An unknown name is a problem at
/// `field[index]`.
fn filters_named(names: &[String], field: &str, problems: &mut Problems) -> Vec<Filter> {
    let mut filters = Vec::new();
    for (i, name) in names.iter().enumerate() {
        match Filter::from_name(name) {
            Some(filter) => filters.push(filter),
            None => {
                let known: Vec<&str> = Filter::ALL.iter().map(|f| f.name()).collect();
                problems.add(
                    format!("{field}[{i}]"),
                    format!("unknown filter {name:?}; use {}", known.join(" or ")),
                );
            }
        }
    }
    filters
}

fn parse_mode(text: &str, field: &str, problems: &mut Problems) -> Option<Mode> {
    match text {
        "sequential" => Some(Mode::Sequential),
        "shuffle" => Some(Mode::Shuffle),
        "weighted" => Some(Mode::Weighted),
        _ => {
            problems.add(
                field,
                format!("unknown mode {text:?}; use sequential, shuffle, weighted or blocks"),
            );
            None
        }
    }
}

/// A block's or a default's mode, which cannot itself be `blocks`.
fn block_mode(text: &str, field: &str, problems: &mut Problems) -> Option<Mode> {
    if text == "blocks" {
        problems.add(field, "cannot itself use mode \"blocks\"");
        return None;
    }
    parse_mode(text, field, problems)
}

/// Checks the `source` or `sources` of a lineup in `mode`, where `prefix` is
/// the lineup's field path: `channel[0]`, `channel[0].block[1]` or
/// `channel[0].default`. A `weighted` lineup takes `sources`, at least one,
/// each with an optional `weight` of 1 to 4294967295 (1 when left out). Any
/// other mode takes one `source` and gives it weight 1. `mode` is `None` when
/// it was already reported as wrong; then only what is present is checked.
fn check_lineup(
    prefix: &str,
    source: Option<&Source>,
    sources: Option<&[RawWeightedSource]>,
    mode: Option<Mode>,
    problems: &mut Problems,
) -> Vec<WeightedSource> {
    let field = |key: &str| format!("{prefix}.{key}");
    match mode {
        Some(Mode::Weighted) => {
            if source.is_some() {
                problems.add(
                    field("source"),
                    "not used with mode \"weighted\"; list the sources, each with its weight, in `sources`",
                );
            }
            match sources {
                Some(list) if !list.is_empty() => {
                    check_weighted_sources(list, &field("sources"), problems)
                }
                _ => {
                    problems.add(
                        field("sources"),
                        "mode \"weighted\" needs at least one source, such as sources = [{ jellyfin_series = \"Name\", weight = 2 }]",
                    );
                    Vec::new()
                }
            }
        }
        Some(_) => {
            if sources.is_some() {
                problems.add(
                    field("sources"),
                    "only used with mode \"weighted\"; other modes take one `source`",
                );
            }
            match source {
                Some(source) => vec![WeightedSource::new(source.clone())],
                None => {
                    if sources.is_none() {
                        problems.add(field("source"), "missing; name the source to play");
                    }
                    Vec::new()
                }
            }
        }
        None => {
            match sources {
                Some(list) => {
                    check_weighted_sources(list, &field("sources"), problems);
                }
                None if source.is_none() => {
                    problems.add(field("source"), "missing; name the source to play");
                }
                None => {}
            }
            Vec::new()
        }
    }
}

/// The entries of a weighted lineup's `sources` at `field`, each with its
/// weight. An entry with a bad weight or a repeated source is a problem and
/// is left out.
fn check_weighted_sources(
    list: &[RawWeightedSource],
    field: &str,
    problems: &mut Problems,
) -> Vec<WeightedSource> {
    let mut checked = Vec::new();
    for (j, entry) in list.iter().enumerate() {
        if let Some(first) = list[..j]
            .iter()
            .position(|other| other.source == entry.source)
        {
            problems.add(
                format!("{field}[{j}]"),
                format!("is the same source as {field}[{first}]; list it once and add the weights"),
            );
        }
        let weight = match entry.weight {
            None => Some(1),
            Some(weight) => match u32::try_from(weight) {
                Ok(weight) if weight >= 1 => Some(weight),
                _ => {
                    problems.add(
                        format!("{field}[{j}].weight"),
                        format!("{weight} is not a weight; use a whole number of at least 1"),
                    );
                    None
                }
            },
        };
        if let Some(weight) = weight {
            checked.push(WeightedSource {
                source: entry.source.clone(),
                weight,
            });
        }
    }
    checked
}

fn positive_duration(text: &str, field: &str, problems: &mut Problems) -> Option<Duration> {
    match parse_duration(text) {
        Ok(d) if d.is_zero() => {
            problems.add(field, "must be longer than zero");
            None
        }
        Ok(d) => Some(d),
        Err(message) => {
            problems.add(field, message);
            None
        }
    }
}

fn parse_day(text: &str) -> Option<Day> {
    Some(match text {
        "mon" => Day::Mon,
        "tue" => Day::Tue,
        "wed" => Day::Wed,
        "thu" => Day::Thu,
        "fri" => Day::Fri,
        "sat" => Day::Sat,
        "sun" => Day::Sun,
        _ => return None,
    })
}

const ALL_DAYS: [Day; 7] = [
    Day::Mon,
    Day::Tue,
    Day::Wed,
    Day::Thu,
    Day::Fri,
    Day::Sat,
    Day::Sun,
];

fn check_blocks(raw: &[RawBlock], channel: usize, problems: &mut Problems) -> Option<Vec<Block>> {
    let before = problems.0.len();
    let mut blocks: Vec<Block> = Vec::new();

    for (j, block) in raw.iter().enumerate() {
        let at = |key: &str| format!("channel[{channel}].block[{j}].{key}");

        let mut days = Vec::new();
        for (k, name) in block.days.iter().enumerate() {
            match parse_day(name) {
                Some(day) => days.push(day),
                None => problems.add(
                    format!("channel[{channel}].block[{j}].days[{k}]"),
                    format!("unknown day {name:?}; use mon, tue, wed, thu, fri, sat or sun"),
                ),
            }
        }
        if block.days.is_empty() {
            days = ALL_DAYS.to_vec();
        }
        days.sort();
        days.dedup();

        let start = parse_minute_of_day(&block.start)
            .map_err(|message| problems.add(at("start"), message))
            .ok();
        let end = parse_minute_of_day(&block.end)
            .map_err(|message| problems.add(at("end"), message))
            .ok();
        if let (Some(start), Some(end)) = (start, end)
            && end <= start
        {
            problems.add(
                at("end"),
                "must be after start; blocks do not cross midnight",
            );
        }

        let mode = block_mode(&block.mode, &at("mode"), problems);
        let sources = check_lineup(
            &format!("channel[{channel}].block[{j}]"),
            block.source.as_ref(),
            block.sources.as_deref(),
            mode,
            problems,
        );

        if let (Some(start), Some(end), Some(mode)) = (start, end, mode)
            && end > start
        {
            let candidate = Block {
                days,
                start_minute: start,
                end_minute: end,
                sources,
                mode,
            };
            if let Some(earlier) = blocks.iter().position(|other| overlaps(other, &candidate)) {
                problems.add(
                    format!("channel[{channel}].block[{j}]"),
                    format!("overlaps block[{earlier}]; blocks must not overlap"),
                );
            }
            blocks.push(candidate);
        }
    }

    (problems.0.len() == before).then_some(blocks)
}

fn overlaps(a: &Block, b: &Block) -> bool {
    a.days.iter().any(|day| b.days.contains(day))
        && a.start_minute < b.end_minute
        && b.start_minute < a.end_minute
}
