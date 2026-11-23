//! The file as written, and the checks that turn it into [`Config`].
//!
//! Values that can be wrong in ways worth naming (modes, durations, times, the
//! time zone) stay strings here so each problem is reported against its key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::duration::{parse_duration, parse_minute_of_day};
use super::{
    Block, Breaks, Channel, Config, Day, InterstitialPool, Mode, Plan, Problem, Source, Tz,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    timezone: String,
    #[serde(default)]
    channel: Vec<RawChannel>,
    #[serde(default)]
    interstitial_pool: Vec<RawPool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChannel {
    number: u32,
    name: String,
    source: Option<Source>,
    mode: String,
    grid: Option<String>,
    breaks: Option<RawBreaks>,
    filler: Option<String>,
    #[serde(default)]
    block: Vec<RawBlock>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBreaks {
    every: String,
    length: String,
    pool: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    #[serde(default)]
    days: Vec<String>,
    start: String,
    end: String,
    source: Source,
    mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPool {
    name: String,
    path: PathBuf,
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

        let mut pools = Vec::new();
        let mut pool_index: HashMap<&str, usize> = HashMap::new();
        for (i, pool) in self.interstitial_pool.iter().enumerate() {
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
            if let Some(channel) = raw.check(i, &pool_index, &mut problems) {
                channels.push(channel);
            }
        }

        match (timezone, problems.0.is_empty()) {
            (Some(timezone), true) => Ok(Config {
                timezone,
                channels,
                pools,
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
            if !pools.contains_key(b.pool.as_str()) {
                problems.add(
                    at("breaks.pool"),
                    format!("no interstitial_pool named {:?}", b.pool),
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
                    pool: b.pool.clone(),
                }),
                _ => None,
            }
        });

        if let Some(filler) = &self.filler
            && !pools.contains_key(filler.as_str())
        {
            problems.add(
                at("filler"),
                format!("no interstitial_pool named {filler:?}"),
            );
        }

        if problems.0.len() > before {
            return None;
        }
        Some(Channel {
            number: self.number,
            name: self.name.clone(),
            plan: plan?,
            grid,
            breaks,
            filler: self.filler.clone(),
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
            let blocks = check_blocks(&self.block, i, problems)?;
            return Some(Plan::Blocks(blocks));
        }

        if !self.block.is_empty() {
            problems.add(at("block"), "blocks need mode = \"blocks\"");
        }
        let mode = parse_mode(&self.mode, &at("mode"), problems);
        match (&self.source, mode) {
            (Some(source), Some(mode)) => Some(Plan::Fixed {
                source: source.clone(),
                mode,
            }),
            (None, _) => {
                problems.add(at("source"), "missing; a channel needs a source");
                None
            }
            _ => None,
        }
    }
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

        let mode = match block.mode.as_str() {
            "blocks" => {
                problems.add(at("mode"), "a block cannot itself use mode \"blocks\"");
                None
            }
            text => parse_mode(text, &at("mode"), problems),
        };

        if let (Some(start), Some(end), Some(mode)) = (start, end, mode)
            && end > start
        {
            let candidate = Block {
                days,
                start_minute: start,
                end_minute: end,
                source: block.source.clone(),
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
