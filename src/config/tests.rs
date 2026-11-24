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
    assert_eq!(config.pools.len(), 3);

    let cartoons = &config.channels[0];
    assert_eq!(cartoons.number, 3);
    assert_eq!(cartoons.name, "SATURDAY CARTOONS");
    assert_eq!(
        cartoons.plan,
        Plan::Fixed {
            source: Source::JellyfinCollection("Cartoons".into()),
            mode: Mode::Shuffle,
            weights: Weights::new(),
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
    assert_eq!(blocks[1].weights.get("Groundhog Day"), Some(&3));
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
source = { jellyfin_tag = "c" }
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

#[test]
fn weights_load_for_a_weighted_channel() {
    let text = with_channel(
        "source = { jellyfin_library = \"M\" }\nmode = \"weighted\"\nweights = { \"Cars\" = 3, \"Up\" = 1 }",
    );
    let config = Config::parse(&text).unwrap();
    let Plan::Fixed { weights, .. } = &config.channels[0].plan else {
        panic!("expected a fixed plan");
    };
    assert_eq!(
        weights,
        &Weights::from([("Cars".into(), 3), ("Up".into(), 1)])
    );
}

#[test]
fn rejects_weights_outside_weighted_and_bad_values() {
    let text = with_channel(&format!("{PLAIN}\nweights = {{ \"Cars\" = 2 }}"));
    assert_eq!(fields(&text), vec!["channel[0].weights"]);

    let text = with_channel(
        "source = { jellyfin_library = \"M\" }\nmode = \"weighted\"\nweights = { \"Cars\" = 0, \"Up\" = -1, \"Big\" = 4294967296 }",
    );
    assert_eq!(
        fields(&text),
        vec![
            "channel[0].weights.\"Big\"",
            "channel[0].weights.\"Cars\"",
            "channel[0].weights.\"Up\"",
        ]
    );
}

#[test]
fn a_blocks_channel_takes_a_default_lineup_and_no_channel_weights() {
    let text = blocks_channel(&format!(
        "default = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"weighted\", weights = {{ \"X\" = 2 }} }}\n{BLOCK_MORNING}"
    ));
    let config = Config::parse(&text).unwrap();
    let Plan::Blocks { default, .. } = &config.channels[0].plan else {
        panic!("expected blocks");
    };
    assert_eq!(
        default,
        &Some(Lineup {
            source: Source::JellyfinTag("d".into()),
            mode: Mode::Weighted,
            weights: Weights::from([("X".into(), 2)]),
        })
    );

    let text = blocks_channel(&format!(
        "weights = {{ \"X\" = 2 }}\ndefault = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"blocks\" }}\n{BLOCK_MORNING}"
    ));
    assert_eq!(
        fields(&text),
        vec!["channel[0].weights", "channel[0].default.mode"]
    );

    let text = with_channel(&format!(
        "{PLAIN}\ndefault = {{ source = {{ jellyfin_tag = \"d\" }}, mode = \"shuffle\" }}"
    ));
    assert_eq!(fields(&text), vec!["channel[0].default"]);
}

#[test]
fn rejects_block_weights_outside_weighted() {
    let block = BLOCK_MORNING.replace(
        "mode = \"shuffle\"",
        "mode = \"shuffle\"\nweights = { \"X\" = 2 }",
    );
    assert_eq!(
        fields(&blocks_channel(&block)),
        vec!["channel[0].block[0].weights"]
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
        ]
    );
    assert_eq!(
        config.channels[0].plan.sources(),
        vec![&Source::JellyfinCollection("Cartoons".into())]
    );
}
