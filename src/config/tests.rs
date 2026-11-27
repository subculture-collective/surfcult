use std::path::Path;
use std::time::Duration;

use super::*;

const EXAMPLE: &str = include_str!("../../config/channels.example.toml");

fn problems(text: &str) -> Vec<Problem> {
    match Config::parse(text) {
        Err(ConfigError::Invalid(problems)) => problems,
        other => panic!("expected validation problems, got {other:?}"),
    }
}

fn fields(text: &str) -> Vec<String> {
    problems(text).into_iter().map(|p| p.field).collect()
}

/// A valid config with one channel and the pools it names, plus `extra` lines
/// appended to the channel table.
fn with_channel(extra: &str) -> String {
    format!(
        r#"
timezone = "UTC"
[[interstitial_pool]]
name = "ads"
path = "/clips/ads"
[[channel]]
number = 1
name = "ONE"
{extra}
"#
    )
}

const PLAIN: &str = "source = { jellyfin_library = \"Movies\" }\nmode = \"shuffle\"";

#[test]
fn the_example_file_loads_into_typed_config() {
    let config = Config::parse(EXAMPLE).expect("example is valid");

    assert_eq!(config.timezone, Tz::America__Chicago);
    assert_eq!(config.channels.len(), 3);
    assert_eq!(config.pools.len(), 4);

    let cartoons = &config.channels[0];
    assert_eq!(cartoons.number, 3);
    assert_eq!(cartoons.name, "SATURDAY CARTOONS");
    assert_eq!(
        cartoons.plan,
        Plan::Fixed {
            sources: vec![WeightedSource::new(Source::JellyfinCollection(
                "Cartoons".into()
            ))],
            mode: Mode::Shuffle,
        }
    );
    assert_eq!(cartoons.grid, Some(Duration::from_secs(30 * 60)));
    assert_eq!(
        cartoons.breaks,
        Some(Breaks {
            every: Duration::from_secs(11 * 60),
            length: Duration::from_secs(2 * 60),
            pool: "ads-90s".into(),
            rotation: Rotation::Order,
            repeat_after: None,
            pad: None,
        })
    );
    assert_eq!(cartoons.filler.as_deref(), Some("filler"));

    let Plan::Blocks { blocks, default } = &config.channels[2].plan else {
        panic!("channel 5 should use blocks");
    };
    assert_eq!(
        default.as_ref().map(|lineup| lineup.mode),
        Some(Mode::Sequential)
    );
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].days, vec![Day::Sat, Day::Sun]);
    assert_eq!((blocks[0].start_minute, blocks[0].end_minute), (360, 720));
    assert_eq!(blocks[1].days.len(), 7);
    assert_eq!(blocks[1].end_minute, 1440);
    assert_eq!(blocks[1].mode, Mode::Weighted);
    assert_eq!(
        blocks[1].sources,
        vec![
            WeightedSource {
                source: Source::JellyfinGenre("Comedy".into()),
                weight: 3
            },
            WeightedSource::new(Source::JellyfinCollection("Cartoon Shorts".into())),
        ]
    );
}

#[test]
fn load_reads_a_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/channels.example.toml");
    assert!(Config::load(&path).is_ok());
}

#[test]
fn load_reports_a_missing_file() {
    let err = Config::load(Path::new("/nonexistent/channels.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Read { .. }));
    assert!(err.to_string().contains("/nonexistent/channels.toml"));
}

#[test]
fn rejects_duplicate_channel_numbers() {
    let text = format!(
        "{}\n[[channel]]\nnumber = 1\nname = \"TWO\"\n{PLAIN}\n",
        with_channel(PLAIN)
    );
    let found = problems(&text);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "channel[1].number");
    assert!(
        found[0].message.contains("channel[0]"),
        "{}",
        found[0].message
    );
}

#[test]
fn rejects_an_unknown_mode_naming_the_field() {
    let found = problems(&with_channel(
        "source = { jellyfin_library = \"Movies\" }\nmode = \"random\"",
    ));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "channel[0].mode");
    assert!(found[0].message.contains("\"random\""));
}

#[test]
fn rejects_an_unknown_time_zone_naming_the_field() {
    let text = with_channel(PLAIN).replace("UTC", "Mars/Olympus_Mons");
    let found = problems(&text);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "timezone");
    assert!(found[0].message.contains("Mars/Olympus_Mons"));
}

fn blocks_channel(blocks: &str) -> String {
    with_channel(&format!("mode = \"blocks\"\n{blocks}"))
}

const BLOCK_MORNING: &str = r#"
[[channel.block]]
days = ["sat"]
start = "06:00"
end = "12:00"
source = { jellyfin_tag = "a" }
mode = "shuffle"
"#;

#[test]
fn rejects_overlapping_blocks_naming_the_field() {
    let overlapping = r#"
[[channel.block]]
days = ["sat", "sun"]
start = "11:00"
end = "13:00"
source = { jellyfin_tag = "b" }
mode = "sequential"
"#;
    let found = problems(&blocks_channel(&format!("{BLOCK_MORNING}{overlapping}")));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "channel[0].block[1]");
    assert!(found[0].message.contains("block[0]"));
}

#[test]
fn blocks_that_touch_or_differ_in_day_do_not_overlap() {
    let touching = r#"
[[channel.block]]
days = ["sat"]
start = "12:00"
end = "18:00"
source = { jellyfin_tag = "b" }
mode = "sequential"

[[channel.block]]
days = ["sun"]
start = "06:00"
end = "12:00"
sources = [{ jellyfin_tag = "c" }]
mode = "weighted"
"#;
    assert!(Config::parse(&blocks_channel(&format!("{BLOCK_MORNING}{touching}"))).is_ok());
}

#[test]
fn rejects_bad_block_fields_naming_each() {
    let bad = r#"
[[channel.block]]
days = ["someday"]
start = "25:00"
end = "06:00"
source = { jellyfin_tag = "a" }
mode = "blocks"
"#;
    let names = fields(&blocks_channel(bad));
    assert_eq!(
        names,
        vec![
            "channel[0].block[0].days[0]",
            "channel[0].block[0].start",
            "channel[0].block[0].mode",
        ]
    );
}

#[test]
fn rejects_a_block_that_ends_before_it_starts() {
    let bad = r#"
[[channel.block]]
start = "20:00"
end = "02:00"
source = { jellyfin_tag = "a" }
mode = "shuffle"
"#;
    assert_eq!(
        fields(&blocks_channel(bad)),
        vec!["channel[0].block[0].end"]
    );
}

#[test]
fn blocks_mode_needs_blocks_and_no_channel_source() {
    assert_eq!(
        fields(&with_channel("mode = \"blocks\"")),
        vec!["channel[0].mode"]
    );
    let text = with_channel(&format!(
        "mode = \"blocks\"\nsource = {{ jellyfin_tag = \"x\" }}\n{BLOCK_MORNING}"
    ));
    assert_eq!(fields(&text), vec!["channel[0].source"]);
}

#[test]
fn a_plain_mode_needs_a_source_and_no_blocks() {
    assert_eq!(
        fields(&with_channel("mode = \"shuffle\"")),
        vec!["channel[0].source"]
    );
    let text = with_channel(&format!("{PLAIN}\n{BLOCK_MORNING}"));
    assert_eq!(fields(&text), vec!["channel[0].block"]);
}

#[test]
fn rejects_bad_grid_and_breaks() {
    let text = with_channel(&format!(
        "{PLAIN}\ngrid = \"0m\"\nbreaks = {{ every = \"5m\", length = \"5m\", pool = \"nope\" }}\nfiller = \"nada\""
    ));
    assert_eq!(
        fields(&text),
        vec![
            "channel[0].grid",
            "channel[0].breaks.pool",
            "channel[0].breaks.length",
            "channel[0].filler",
        ]
    );
}

#[test]
fn rejects_duplicate_pool_names() {
    let text = format!(
        "{}\n[[interstitial_pool]]\nname = \"ads\"\npath = \"/other\"\n",
        with_channel(PLAIN)
    );
    assert_eq!(fields(&text), vec!["interstitial_pool[1].name"]);
}

/// `with_channel` with `pool_keys` added to the `ads` pool and `breaks`
/// set from the inline table body `breaks`, plus an `idents` pool.
fn with_break_settings(pool_keys: &str, breaks: &str) -> String {
    let text = with_channel(&format!("{PLAIN}\nbreaks = {{ {breaks} }}")).replace(
        "path = \"/clips/ads\"",
        &format!("path = \"/clips/ads\"\n{pool_keys}"),
    );
    format!("{text}\n[[interstitial_pool]]\nname = \"idents\"\npath = \"/clips/idents\"\n")
}

const BREAKS: &str = "every = \"10m\", length = \"2m\", pool = \"ads\"";

#[test]
fn break_settings_load_into_the_pool_and_the_channel_breaks() {
    let text = with_break_settings(
        "rotation = \"shuffle\"\nrepeat_after = \"2h\"",
        &format!("{BREAKS}, pad = \"idents\""),
    );
    let config = Config::parse(&text).unwrap();

    let ads = &config.pools[0];
    assert_eq!(ads.rotation, Rotation::Shuffle);
    assert_eq!(ads.repeat_after, Some(Duration::from_secs(2 * 3600)));
    assert_eq!(
        config.channels[0].breaks,
        Some(Breaks {
            every: Duration::from_secs(10 * 60),
            length: Duration::from_secs(2 * 60),
            pool: "ads".into(),
            rotation: Rotation::Shuffle,
            repeat_after: Some(Duration::from_secs(2 * 3600)),
            pad: Some("idents".into()),
        })
    );

    // The defaults: pool order, no repeat limit, no pad pool.
    let config = Config::parse(&with_break_settings("rotation = \"order\"", BREAKS)).unwrap();
    let breaks = config.channels[0].breaks.as_ref().unwrap();
    assert_eq!(
        (breaks.rotation, breaks.repeat_after, breaks.pad.as_deref()),
        (Rotation::Order, None, None)
    );
    let config = Config::parse(&with_break_settings("", BREAKS)).unwrap();
    assert_eq!(config.pools[1].rotation, Rotation::Order);
    assert_eq!(config.pools[1].repeat_after, None);
}

#[test]
fn rejects_bad_break_settings_naming_each_field() {
    for (keys, field, needle) in [
        (
            "rotation = \"random\"",
            "interstitial_pool[0].rotation",
            "\"random\"",
        ),
        (
            "repeat_after = \"2 hours\"",
            "interstitial_pool[0].repeat_after",
            "unit",
        ),
        (
            "repeat_after = \"0m\"",
            "interstitial_pool[0].repeat_after",
            "zero",
        ),
        (
            "repeat_after = \"-5m\"",
            "interstitial_pool[0].repeat_after",
            "-5m",
        ),
    ] {
        let found = problems(&with_break_settings(keys, BREAKS));
        assert_eq!(found.len(), 1, "{keys}: {found:?}");
        assert_eq!(found[0].field, field, "{keys}");
        assert!(
            found[0].message.contains(needle),
            "{keys}: {}",
            found[0].message
        );
    }

    let found = problems(&with_break_settings(
        "",
        &format!("{BREAKS}, pad = \"bumpers\""),
    ));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "channel[0].breaks.pad");
    assert!(found[0].message.contains("\"bumpers\""));
}

#[test]
fn rotation_and_repeat_after_are_refused_on_a_pool_that_is_never_a_break_pool() {
    let text = with_break_settings("", &format!("{BREAKS}, pad = \"idents\"")).replace(
        "path = \"/clips/idents\"",
        "path = \"/clips/idents\"\nrotation = \"shuffle\"\nrepeat_after = \"1h\"",
    );
    let found = problems(&text);
    let fields: Vec<_> = found.iter().map(|p| p.field.as_str()).collect();
    assert_eq!(
        fields,
        vec![
            "interstitial_pool[1].rotation",
            "interstitial_pool[1].repeat_after"
        ]
    );
    assert!(found[0].message.contains("\"idents\""));
}

#[test]
fn unknown_break_and_pool_keys_are_parse_errors() {
    for text in [
        with_break_settings("repeat = \"2h\"", BREAKS),
        with_break_settings("", &format!("{BREAKS}, pad_pool = \"idents\"")),
        with_break_settings("rotation = 1", BREAKS),
    ] {
        assert!(
            matches!(Config::parse(&text), Err(ConfigError::Parse(_))),
            "{text}"
        );
    }
}

#[test]
fn reports_every_problem_at_once() {
    let text = with_channel("source = { jellyfin_library = \"M\" }\nmode = \"random\"")
        .replace("UTC", "Nowhere/Land");
    assert_eq!(fields(&text), vec!["timezone", "channel[0].mode"]);
}

#[test]
fn malformed_toml_and_unknown_keys_are_parse_errors_with_a_location() {
    for text in [
        "timezone = ",
        &with_channel(&format!("{PLAIN}\ncolour = \"red\"")),
        &with_channel("source = { jellyfin_nonsense = \"x\" }\nmode = \"shuffle\""),
    ] {
        match Config::parse(text) {
            Err(ConfigError::Parse(message)) => assert!(!message.is_empty()),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }
}

#[test]
fn display_lists_each_problem_with_its_field() {
    let err = Config::parse(&with_channel(
        "source = { jellyfin_library = \"M\" }\nmode = \"x\"",
    ))
    .unwrap_err();
    let shown = err.to_string();
    assert!(shown.starts_with("channel[0].mode: "), "{shown}");
}

/// The sources of a weighted fixed channel written as `entries`.
fn weighted_channel(entries: &str) -> String {
    with_channel(&format!("mode = \"weighted\"\nsources = [{entries}]"))
}

fn fixed_sources(config: &Config) -> &[WeightedSource] {
    let Plan::Fixed { sources, .. } = &config.channels[0].plan else {
        panic!("expected a fixed plan");
    };
    sources
}

fn ws(source: Source, weight: u32) -> WeightedSource {
    WeightedSource { source, weight }
}

#[test]
fn a_weighted_channel_lists_sources_with_weights() {
    let text = weighted_channel(
        r#"
  { jellyfin_series = "The Simpsons", weight = 3 },
  { jellyfin_series = "Futurama" },
  { weight = 2, jellyfin_collection = "Cartoon Shorts" },
  { jellyfin_library = "L", weight = 1 },
  { jellyfin_tag = "t", weight = 4294967295 },
  { jellyfin_genre = "g", weight = 5 },
"#,
    );
    let config = Config::parse(&text).unwrap();
    assert_eq!(
        fixed_sources(&config),
        [
            ws(Source::JellyfinSeries("The Simpsons".into()), 3),
            ws(Source::JellyfinSeries("Futurama".into()), 1),
            ws(Source::JellyfinCollection("Cartoon Shorts".into()), 2),
            ws(Source::JellyfinLibrary("L".into()), 1),
            ws(Source::JellyfinTag("t".into()), u32::MAX),
            ws(Source::JellyfinGenre("g".into()), 5),
        ]
    );
    let Plan::Fixed { mode, .. } = &config.channels[0].plan else {
        unreachable!()
    };
    assert_eq!(*mode, Mode::Weighted);
}

#[test]
fn other_modes_keep_one_source_with_weight_one() {
    let config = Config::parse(&with_channel(PLAIN)).unwrap();
    assert_eq!(
        fixed_sources(&config),
        [ws(Source::JellyfinLibrary("Movies".into()), 1)]
    );
}

#[test]
fn a_weighted_lineup_needs_at_least_one_source() {
    assert_eq!(
        fields(&with_channel("mode = \"weighted\"")),
        vec!["channel[0].sources"]
    );
    assert_eq!(
        fields(&with_channel("mode = \"weighted\"\nsources = []")),
        vec!["channel[0].sources"]
    );
    // The old form is not accepted: a weighted channel lists `sources`.
    assert_eq!(
        fields(&with_channel(
            "source = { jellyfin_library = \"M\" }\nmode = \"weighted\""
        )),
        vec!["channel[0].source", "channel[0].sources"]
    );
}

#[test]
fn rejects_bad_weights_naming_each_entry() {
    let text = weighted_channel(
        r#"
  { jellyfin_tag = "a", weight = 0 },
  { jellyfin_tag = "b", weight = 2 },
  { jellyfin_tag = "c", weight = -1 },
  { jellyfin_tag = "d", weight = 4294967296 },
"#,
    );
    assert_eq!(
        fields(&text),
        vec![
            "channel[0].sources[0].weight",
            "channel[0].sources[2].weight",
            "channel[0].sources[3].weight",
        ]
    );
    let found = problems(&text);
    assert!(found[0].message.contains("whole number of at least 1"));
}

#[test]
fn rejects_the_same_source_twice() {
    let text = weighted_channel(
        r#"
  { jellyfin_tag = "a", weight = 2 },
  { jellyfin_tag = "b" },
  { jellyfin_tag = "a" },
"#,
    );
    let found = problems(&text);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].field, "channel[0].sources[2]");
    assert!(found[0].message.contains("sources[0]"));
}

#[test]
fn other_modes_reject_sources() {
    for mode in ["sequential", "shuffle"] {
        let text = with_channel(&format!(
            "mode = \"{mode}\"\nsources = [{{ jellyfin_tag = \"a\", weight = 2 }}, {{ jellyfin_tag = \"b\" }}]"
        ));
        assert_eq!(fields(&text), vec!["channel[0].sources"], "{mode}");
    }
    // A weight beside a plain `source` is an unknown key.
    let text = with_channel("source = { jellyfin_tag = \"a\", weight = 2 }\nmode = \"shuffle\"");
    assert!(matches!(Config::parse(&text), Err(ConfigError::Parse(_))));
}

#[test]
fn a_source_entry_with_an_unknown_or_missing_key_is_a_parse_error() {
    for entry in [
        "{ jellyfin_tag = \"a\", colour = \"red\" }",
        "{ jellyfin_nonsense = \"a\" }",
        "{ weight = 2 }",
        "{ jellyfin_tag = \"a\", jellyfin_series = \"b\" }",
        "{ jellyfin_tag = \"a\", weight = 2, weight = 3 }",
        "{ jellyfin_tag = \"a\", weight = 1.5 }",
        "{ jellyfin_tag = \"a\", weight = \"3\" }",
        "{ jellyfin_tag = 4 }",
        "\"a\"",
    ] {
        match Config::parse(&weighted_channel(entry)) {
            Err(ConfigError::Parse(message)) => assert!(!message.is_empty(), "{entry}"),
            other => panic!("{entry}: expected a parse error, got {other:?}"),
        }
    }
}

#[test]
fn the_old_weights_table_is_an_unknown_key() {
    for text in [
        with_channel(
            "source = { jellyfin_library = \"M\" }\nmode = \"weighted\"\nweights = { \"Cars\" = 3 }",
        ),
        blocks_channel(
            &BLOCK_MORNING.replace("\"shuffle\"", "\"shuffle\"\nweights = { \"X\" = 2 }"),
        ),
        blocks_channel(&format!(
            "default = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"shuffle\", weights = {{ \"X\" = 2 }} }}\n{BLOCK_MORNING}"
        )),
    ] {
        assert!(
            matches!(Config::parse(&text), Err(ConfigError::Parse(_))),
            "{text}"
        );
    }
}

#[test]
fn a_blocks_channel_takes_a_default_lineup_and_sources_only_on_blocks() {
    let text = blocks_channel(&format!(
        "default = {{ sources = [{{ jellyfin_tag = \"d\", weight = 2 }}, {{ jellyfin_tag = \"e\" }}], mode = \"weighted\" }}\n{BLOCK_MORNING}"
    ));
    let config = Config::parse(&text).unwrap();
    let Plan::Blocks { default, .. } = &config.channels[0].plan else {
        panic!("expected blocks");
    };
    assert_eq!(
        default,
        &Some(Lineup {
            sources: vec![
                ws(Source::JellyfinTag("d".into()), 2),
                ws(Source::JellyfinTag("e".into()), 1),
            ],
            mode: Mode::Weighted,
        })
    );

    let text = blocks_channel(&format!(
        "sources = [{{ jellyfin_tag = \"x\" }}]\ndefault = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"blocks\" }}\n{BLOCK_MORNING}"
    ));
    assert_eq!(
        fields(&text),
        vec!["channel[0].sources", "channel[0].default.mode"]
    );

    let text = with_channel(&format!(
        "{PLAIN}\ndefault = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"shuffle\" }}"
    ));
    assert_eq!(fields(&text), vec!["channel[0].default"]);
}

#[test]
fn a_default_lineup_checks_its_sources_like_a_channel() {
    let weighted =
        |lineup: &str| blocks_channel(&format!("default = {{ {lineup} }}\n{BLOCK_MORNING}"));
    assert_eq!(
        fields(&weighted("mode = \"weighted\"")),
        vec!["channel[0].default.sources"]
    );
    assert_eq!(
        fields(&weighted(
            "mode = \"weighted\", sources = [{ jellyfin_tag = \"d\", weight = 0 }]"
        )),
        vec!["channel[0].default.sources[0].weight"]
    );
    assert_eq!(
        fields(&weighted(
            "mode = \"shuffle\", sources = [{ jellyfin_tag = \"d\" }]"
        )),
        vec!["channel[0].default.sources"]
    );
    assert_eq!(
        fields(&weighted("mode = \"shuffle\"")),
        vec!["channel[0].default.source"]
    );
}

#[test]
fn a_weighted_block_lists_sources_and_other_blocks_take_one() {
    let weighted = r#"
[[channel.block]]
start = "12:00"
end = "18:00"
sources = [{ jellyfin_tag = "w", weight = 3 }, { jellyfin_series = "S" }]
mode = "weighted"
"#;
    let config = Config::parse(&blocks_channel(&format!("{BLOCK_MORNING}{weighted}"))).unwrap();
    let Plan::Blocks { blocks, .. } = &config.channels[0].plan else {
        panic!("expected blocks");
    };
    assert_eq!(blocks[0].sources, [ws(Source::JellyfinTag("a".into()), 1)]);
    assert_eq!(
        blocks[1].sources,
        [
            ws(Source::JellyfinTag("w".into()), 3),
            ws(Source::JellyfinSeries("S".into()), 1),
        ]
    );

    // Field paths name the block and the entry.
    let bad = weighted.replace("weight = 3", "weight = 0");
    assert_eq!(
        fields(&blocks_channel(&format!("{BLOCK_MORNING}{bad}"))),
        vec!["channel[0].block[1].sources[0].weight"]
    );
    let shuffle_with_sources = BLOCK_MORNING.replace(
        "source = { jellyfin_tag = \"a\" }",
        "sources = [{ jellyfin_tag = \"a\" }]",
    );
    assert_eq!(
        fields(&blocks_channel(&shuffle_with_sources)),
        vec!["channel[0].block[0].sources"]
    );
    let no_source = BLOCK_MORNING.replace("source = { jellyfin_tag = \"a\" }\n", "");
    assert_eq!(
        fields(&blocks_channel(&no_source)),
        vec!["channel[0].block[0].source"]
    );
}

#[test]
fn sources_lists_each_source_once_in_config_order() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(
        config.channels[2].plan.sources(),
        vec![
            &Source::JellyfinCollection("Cartoons".into()),
            &Source::JellyfinGenre("Comedy".into()),
            &Source::JellyfinCollection("Cartoon Shorts".into()),
        ]
    );
    assert_eq!(
        config.channels[0].plan.sources(),
        vec![&Source::JellyfinCollection("Cartoons".into())]
    );
}

#[test]
fn a_sign_off_loads_its_times_pool_and_clip() {
    let text = with_channel(&format!(
        "{PLAIN}\nsign_off = {{ at = \"23:30\", pool = \"ads\", clip = \"anthem.mp4\", until = \"06:00\" }}"
    ));
    let config = Config::parse(&text).unwrap();
    assert_eq!(
        config.channels[0].sign_off,
        Some(SignOff {
            at_minute: 23 * 60 + 30,
            until_minute: 6 * 60,
            pool: "ads".into(),
            clip: Some("anthem.mp4".into()),
        })
    );

    let text = with_channel(&format!(
        "{PLAIN}\nsign_off = {{ at = \"01:00\", pool = \"ads\", until = \"05:00\" }}"
    ));
    let sign_off = Config::parse(&text).unwrap().channels[0].sign_off.clone();
    assert_eq!(sign_off.and_then(|s| s.clip), None);
    let plain = Config::parse(&with_channel(PLAIN)).unwrap();
    assert_eq!(plain.channels[0].sign_off, None);
}

#[test]
fn rejects_bad_sign_off_fields_naming_each() {
    let sign_off = |fields: &str| with_channel(&format!("{PLAIN}\nsign_off = {{ {fields} }}"));
    for (text, field) in [
        (
            sign_off("at = \"23:00\", pool = \"nope\", until = \"06:00\""),
            "channel[0].sign_off.pool",
        ),
        (
            sign_off("at = \"23:00\", pool = \"ads\", clip = \"x/anthem.mp4\", until = \"06:00\""),
            "channel[0].sign_off.clip",
        ),
        (
            sign_off("at = \"23:00\", pool = \"ads\", clip = \"..\", until = \"06:00\""),
            "channel[0].sign_off.clip",
        ),
        (
            sign_off("at = \"23:00\", pool = \"ads\", until = \"23:00\""),
            "channel[0].sign_off.until",
        ),
        (
            sign_off("at = \"11pm\", pool = \"ads\", until = \"06:00\""),
            "channel[0].sign_off.at",
        ),
        (
            sign_off("at = \"23:00\", pool = \"ads\", until = \"24:00\""),
            "channel[0].sign_off.until",
        ),
    ] {
        assert_eq!(fields(&text), vec![field], "{text}");
    }

    for text in [
        sign_off("at = \"23:00\", pool = \"ads\""),
        sign_off("at = \"23:00\", pool = \"ads\", until = \"06:00\", days = []"),
    ] {
        assert!(
            matches!(Config::parse(&text), Err(ConfigError::Parse(_))),
            "{text}"
        );
    }
}

// Display filters

fn with_display(table: &str) -> String {
    format!("{}\n[display]\n{table}\n", with_channel(PLAIN))
}

#[test]
fn without_a_display_table_the_card_gets_the_crt_shader_and_the_glitch() {
    let config = Config::parse(&with_channel(PLAIN)).unwrap();
    assert_eq!(config.display, Display::default());
    assert_eq!(config.display.filters, []);
    assert_eq!(
        config.display.standby_filters,
        [Filter::Crt, Filter::Glitch]
    );
    assert_eq!(config.display.glitch_every, Duration::from_secs(20));
    // An empty table means the same.
    assert_eq!(
        Config::parse(&with_display("")).unwrap().display,
        Display::default()
    );
}

#[test]
fn the_display_table_sets_filters_and_the_glitch_rate() {
    let config = Config::parse(&with_display(
        "filters = [\"crt\"]\nstandby_filters = [\"glitch\"]\nglitch_every = \"1m\"",
    ))
    .unwrap();
    assert_eq!(
        config.display,
        Display {
            filters: vec![Filter::Crt],
            standby_filters: vec![Filter::Glitch],
            glitch_every: Duration::from_secs(60),
        }
    );
    // An empty list turns the card's filters off.
    let off = Config::parse(&with_display("standby_filters = []")).unwrap();
    assert_eq!(off.display.standby_filters, []);
}

#[test]
fn rejects_unknown_filters_and_bad_glitch_rates_naming_each() {
    let found = problems(&with_display(
        "filters = [\"crt\", \"vhs\"]\nstandby_filters = [\"Glitch\"]\nglitch_every = \"20\"",
    ));
    let names: Vec<&str> = found.iter().map(|p| p.field.as_str()).collect();
    assert_eq!(
        names,
        [
            "display.filters[1]",
            "display.standby_filters[0]",
            "display.glitch_every",
        ]
    );
    assert!(found[0].message.contains("\"vhs\""), "{}", found[0].message);
    assert!(found[0].message.contains("crt or glitch"));
    assert_eq!(
        fields(&with_display("glitch_every = \"0s\"")),
        ["display.glitch_every"]
    );
}

#[test]
fn the_commented_display_table_in_the_example_gives_the_defaults() {
    let mut text = EXAMPLE.to_owned();
    for line in [
        "[display]",
        "filters = []",
        "standby_filters = [\"crt\", \"glitch\"]",
        "glitch_every = \"20s\"",
    ] {
        let commented = format!("# {line}\n");
        assert!(text.contains(&commented), "{line}");
        text = text.replace(&commented, &format!("{line}\n"));
    }
    assert_eq!(Config::parse(&text).unwrap().display, Display::default());
}

#[test]
fn rejects_unknown_display_keys_as_a_parse_error() {
    let err = Config::parse(&with_display("shader = \"crt\"")).unwrap_err();
    assert!(matches!(err, ConfigError::Parse(_)), "{err:?}");
    assert!(err.to_string().contains("shader"), "{err}");
}

#[test]
fn the_tune_in_pool_defaults_to_tune_in_and_need_not_exist() {
    let config = Config::parse(&with_channel(PLAIN)).unwrap();
    assert_eq!(config.tune_in_pool, "tune-in");
    assert_eq!(config.tune_in_pool, DEFAULT_TUNE_IN_POOL);
}

#[test]
fn a_tune_in_pool_must_name_an_interstitial_pool() {
    let text = with_channel(PLAIN).replace(
        "timezone = \"UTC\"",
        "timezone = \"UTC\"\ntune_in_pool = \"ads\"",
    );
    assert_eq!(Config::parse(&text).unwrap().tune_in_pool, "ads");
    let text = text.replace("tune_in_pool = \"ads\"", "tune_in_pool = \"bumpers\"");
    assert_eq!(
        problems(&text),
        [Problem {
            field: "tune_in_pool".into(),
            message: "no interstitial_pool named \"bumpers\"".into(),
        }]
    );
}

#[test]
fn without_a_keys_table_the_default_keys_apply() {
    let config = Config::parse(&with_channel(PLAIN)).unwrap();
    assert_eq!(config.keys, KeyMap::default());
}

#[test]
fn the_keys_table_overrides_the_named_actions() {
    let config = Config::parse(
        "timezone = \"UTC\"\n[keys]\nchannel_up = [\"KEY_RIGHT\"]\ndigit_1 = [\"KEY_A\"]\n",
    )
    .unwrap();
    assert_eq!(config.keys.key(106), Some(crate::input::Key::Next));
    assert_eq!(config.keys.key(103), None);
    assert_eq!(config.keys.key(30), Some(crate::input::Key::Digit(1)));
    assert_eq!(config.keys.key(108), Some(crate::input::Key::Previous));
}

#[test]
fn rejects_unknown_actions_and_key_names_naming_the_field() {
    let text = "timezone = \"UTC\"\n[keys]\nzap = [\"KEY_UP\"]\nguide = [\"KEY_G\", \"KEY_WAT\"]\nmenu = [\"KEY_G\"]\n";
    assert_eq!(fields(text), ["keys.guide[1]", "keys.menu[0]", "keys.zap"]);
}

#[test]
fn rejects_keys_that_are_not_lists_of_names() {
    assert!(matches!(
        Config::parse("timezone = \"UTC\"\n[keys]\nguide = \"KEY_G\"\n"),
        Err(ConfigError::Parse(_))
    ));
}
