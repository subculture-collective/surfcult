use std::path::PathBuf;
use std::process::Command;

fn run(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_surfcult"))
        .args(["check-config", file])
        .output()
        .expect("run surfcult")
}

#[test]
fn accepts_the_example_file() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/channels.example.toml");
    let out = run(example.to_str().unwrap());
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
}

#[test]
fn exits_non_zero_on_an_invalid_file_and_names_the_field() {
    let dir = std::env::temp_dir().join(format!("surfcult-check-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bad.toml");
    std::fs::write(&file, "timezone = \"Mars/Base\"\n").unwrap();

    let out = run(file.to_str().unwrap());
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("timezone:"));
}

#[test]
fn exits_non_zero_when_the_file_is_missing() {
    let out = run("/nonexistent/channels.toml");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn prints_usage_without_arguments() {
    let out = Command::new(env!("CARGO_BIN_EXE_surfcult"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
