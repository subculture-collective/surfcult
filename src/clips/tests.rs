//! No test here needs ffprobe: durations come from a fake prober, and the
//! process handling is checked with small shell scripts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::probe::parse_seconds;
use super::*;
use crate::config::InterstitialPool;

/// Answers by file name and records every path it was asked about. A name it
/// does not know fails like a file ffprobe cannot read.
#[derive(Default)]
struct FakeProber {
    secs: Mutex<HashMap<String, f64>>,
    calls: Mutex<Vec<PathBuf>>,
}

impl FakeProber {
    fn set(&self, name: &str, secs: f64) {
        self.secs.lock().unwrap().insert(name.to_string(), secs);
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn reset_calls(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl Prober for FakeProber {
    fn duration(&self, path: &Path) -> Result<Duration, ProbeError> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        let name = path.file_name().unwrap().to_string_lossy();
        match self.secs.lock().unwrap().get(name.as_ref()) {
            Some(secs) => Ok(Duration::from_secs_f64(*secs)),
            None => Err(ProbeError::Failed(Some(1))),
        }
    }
}

fn pool(name: &str, dir: &Path) -> InterstitialPool {
    InterstitialPool {
        name: name.to_string(),
        path: dir.to_path_buf(),
    }
}

fn touch(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn names(pool: &Pool) -> Vec<String> {
    pool.clips()
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn a_pool_lists_its_clips_with_durations_sorted_by_name() {
    let dir = TempDir::new().unwrap();
    for name in ["c.mp4", "a.mp4", "b.mp4"] {
        touch(dir.path(), name, "x");
    }
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 10.0);
    prober.set("b.mp4", 20.5);
    prober.set("c.mp4", 30.0);

    let library = ClipLibrary::scan(&[pool("ads", dir.path())], prober);

    let ads = library.pool("ads").unwrap();
    assert_eq!(names(ads), ["a.mp4", "b.mp4", "c.mp4"]);
    assert_eq!(ads.clips()[1].duration, Duration::from_millis(20_500));
    assert_eq!(ads.total_duration(), Duration::from_millis(60_500));
    assert_eq!(library.issues().count(), 0);
    assert_eq!(ads.name(), "ads");
    assert_eq!(ads.dir(), dir.path());
}

#[test]
fn pools_keep_configuration_order_and_are_found_by_name() {
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    touch(two.path(), "x.mp4", "x");
    let prober = FakeProber::default();
    prober.set("x.mp4", 5.0);

    let library = ClipLibrary::scan(
        &[pool("zeta", one.path()), pool("alpha", two.path())],
        prober,
    );

    let order: Vec<_> = library.pools().iter().map(Pool::name).collect();
    assert_eq!(order, ["zeta", "alpha"]);
    assert!(library.pool("zeta").unwrap().clips().is_empty());
    assert_eq!(library.pool("alpha").unwrap().clips().len(), 1);
    assert!(library.pool("missing").is_none());
}

#[test]
fn unreadable_and_zero_length_files_are_skipped_and_reported() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "good.mp4", "x");
    touch(dir.path(), "broken.mp4", "x");
    touch(dir.path(), "empty.mp4", "x");
    let prober = FakeProber::default();
    prober.set("good.mp4", 15.0);
    prober.set("empty.mp4", 0.0);

    let library = ClipLibrary::scan(&[pool("ads", dir.path())], prober);

    let ads = library.pool("ads").unwrap();
    assert_eq!(names(ads), ["good.mp4"]);
    let issues = ads.issues();
    assert_eq!(issues.len(), 2);
    let broken = &issues[0];
    assert_eq!(broken.pool, "ads");
    assert!(broken.path.ends_with("broken.mp4"));
    assert!(matches!(&broken.kind, IssueKind::Probe(why) if why.contains("cannot read")));
    assert!(issues[1].path.ends_with("empty.mp4"));
    assert!(matches!(&issues[1].kind, IssueKind::Probe(why) if why.contains("positive duration")));
}

#[test]
fn a_missing_pool_directory_is_an_empty_pool_with_an_issue() {
    let dir = TempDir::new().unwrap();
    let gone = dir.path().join("not-there");
    let ok = TempDir::new().unwrap();
    touch(ok.path(), "a.mp4", "x");
    let prober = FakeProber::default();
    prober.set("a.mp4", 5.0);

    let library = ClipLibrary::scan(&[pool("lost", &gone), pool("ok", ok.path())], prober);

    let lost = library.pool("lost").unwrap();
    assert!(lost.clips().is_empty());
    assert_eq!(lost.total_duration(), Duration::ZERO);
    assert_eq!(lost.issues().len(), 1);
    assert!(matches!(lost.issues()[0].kind, IssueKind::Directory(_)));
    assert_eq!(library.pool("ok").unwrap().clips().len(), 1);
}

#[test]
fn a_file_given_as_the_pool_directory_is_an_empty_pool_with_an_issue() {
    let dir = TempDir::new().unwrap();
    let file = touch(dir.path(), "not-a-dir", "x");

    let library = ClipLibrary::scan(&[pool("odd", &file)], FakeProber::default());

    assert!(library.pool("odd").unwrap().clips().is_empty());
    assert!(matches!(
        library.pool("odd").unwrap().issues()[0].kind,
        IssueKind::Directory(_)
    ));
}

#[test]
fn subdirectories_and_hidden_files_are_ignored_without_a_report() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "a.mp4", "x");
    touch(dir.path(), ".DS_Store", "x");
    fs::create_dir(dir.path().join("nested")).unwrap();
    touch(&dir.path().join("nested"), "deep.mp4", "x");
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 5.0);
    prober.set("deep.mp4", 5.0);

    let library = ClipLibrary::scan(&[pool("p", dir.path())], prober.clone());

    assert_eq!(names(library.pool("p").unwrap()), ["a.mp4"]);
    assert_eq!(library.issues().count(), 0);
    assert_eq!(prober.calls(), ["a.mp4"]);
}

#[test]
fn a_rescan_picks_up_added_and_removed_clips() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "a.mp4", "x");
    touch(dir.path(), "b.mp4", "x");
    let prober = Arc::new(FakeProber::default());
    for name in ["a.mp4", "b.mp4", "c.mp4"] {
        prober.set(name, 10.0);
    }
    let mut library = ClipLibrary::scan(&[pool("p", dir.path())], prober);
    assert_eq!(names(library.pool("p").unwrap()), ["a.mp4", "b.mp4"]);

    fs::remove_file(dir.path().join("a.mp4")).unwrap();
    touch(dir.path(), "c.mp4", "x");
    library.rescan();

    let p = library.pool("p").unwrap();
    assert_eq!(names(p), ["b.mp4", "c.mp4"]);
    assert_eq!(p.total_duration(), secs(20));
}

#[test]
fn a_rescan_does_not_probe_an_unchanged_file_again() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "a.mp4", "x");
    touch(dir.path(), "bad.mp4", "x");
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 10.0);
    prober.set("b.mp4", 10.0);
    let mut library = ClipLibrary::scan(&[pool("p", dir.path())], prober.clone());
    assert_eq!(prober.calls(), ["a.mp4", "bad.mp4"]);

    prober.reset_calls();
    touch(dir.path(), "b.mp4", "x");
    library.rescan();
    library.rescan();

    // Only the new file was probed, once. The unchanged good file and the
    // unchanged bad file came from the cache, and the bad one is still
    // reported.
    assert_eq!(prober.calls(), ["b.mp4"]);
    let p = library.pool("p").unwrap();
    assert_eq!(names(p), ["a.mp4", "b.mp4"]);
    assert_eq!(p.issues().len(), 1);
}

#[test]
fn a_changed_file_is_probed_again() {
    let dir = TempDir::new().unwrap();
    let path = touch(dir.path(), "a.mp4", "x");
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 10.0);
    let mut library = ClipLibrary::scan(&[pool("p", dir.path())], prober.clone());

    // A different size is enough, even if the modified time did not move.
    fs::write(&path, "a longer file").unwrap();
    prober.set("a.mp4", 25.0);
    prober.reset_calls();
    library.rescan();

    assert_eq!(prober.calls(), ["a.mp4"]);
    assert_eq!(library.pool("p").unwrap().clips()[0].duration, secs(25));
}

#[test]
fn a_removed_then_restored_file_is_probed_again() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "a.mp4", "x");
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 10.0);
    let mut library = ClipLibrary::scan(&[pool("p", dir.path())], prober.clone());

    fs::remove_file(dir.path().join("a.mp4")).unwrap();
    library.rescan();
    assert!(library.pool("p").unwrap().clips().is_empty());

    touch(dir.path(), "a.mp4", "x");
    prober.reset_calls();
    library.rescan();
    assert_eq!(prober.calls(), ["a.mp4"]);
}

#[test]
fn a_pool_directory_that_appears_later_is_picked_up_by_a_rescan() {
    let base = TempDir::new().unwrap();
    let dir = base.path().join("clips");
    let prober = Arc::new(FakeProber::default());
    prober.set("a.mp4", 10.0);
    let mut library = ClipLibrary::scan(&[pool("p", &dir)], prober);
    assert_eq!(library.issues().count(), 1);

    fs::create_dir(&dir).unwrap();
    touch(&dir, "a.mp4", "x");
    library.rescan();

    assert_eq!(names(library.pool("p").unwrap()), ["a.mp4"]);
    assert_eq!(library.issues().count(), 0);
}

#[test]
fn confine_accepts_files_inside_the_directory() {
    let dir = TempDir::new().unwrap();
    let file = touch(dir.path(), "a.mp4", "x");

    let canonical = confine(dir.path(), &file).unwrap();

    assert_eq!(canonical, file.canonicalize().unwrap());
}

#[test]
fn confine_refuses_the_directory_itself_and_files_next_to_it() {
    let base = TempDir::new().unwrap();
    let dir = base.path().join("clips");
    fs::create_dir(&dir).unwrap();
    let sibling = touch(base.path(), "secret.txt", "x");
    // A directory whose name starts with the pool's name is not inside it.
    let lookalike = base.path().join("clips-extra");
    fs::create_dir(&lookalike).unwrap();
    let lookalike_file = touch(&lookalike, "a.mp4", "x");

    assert!(matches!(confine(&dir, &dir), Err(PathRefused::Outside)));
    assert!(matches!(confine(&dir, &sibling), Err(PathRefused::Outside)));
    assert!(matches!(
        confine(&dir, &lookalike_file),
        Err(PathRefused::Outside)
    ));
}

#[test]
fn confine_refuses_dot_dot_paths_that_leave_the_directory() {
    let base = TempDir::new().unwrap();
    let dir = base.path().join("clips");
    fs::create_dir(&dir).unwrap();
    touch(base.path(), "secret.txt", "x");

    let sneaky = dir.join("..").join("secret.txt");
    assert!(matches!(confine(&dir, &sneaky), Err(PathRefused::Outside)));

    // `..` that stays inside is fine.
    fs::create_dir(dir.join("sub")).unwrap();
    let inside = touch(&dir, "a.mp4", "x");
    let roundabout = dir.join("sub").join("..").join("a.mp4");
    assert_eq!(
        confine(&dir, &roundabout).unwrap(),
        inside.canonicalize().unwrap()
    );
}

#[test]
fn confine_refuses_a_path_that_does_not_exist() {
    let dir = TempDir::new().unwrap();

    let result = confine(dir.path(), &dir.path().join("nothing.mp4"));

    assert!(matches!(result, Err(PathRefused::Unresolvable(_))));
}

#[cfg(unix)]
mod symlinks {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn confine_refuses_a_symlink_that_points_outside() {
        let base = TempDir::new().unwrap();
        let dir = base.path().join("clips");
        fs::create_dir(&dir).unwrap();
        let outside = touch(base.path(), "secret.txt", "x");
        symlink(&outside, dir.join("link.mp4")).unwrap();

        let result = confine(&dir, &dir.join("link.mp4"));

        assert!(matches!(result, Err(PathRefused::Outside)));
    }

    #[test]
    fn confine_accepts_a_symlink_that_stays_inside() {
        let dir = TempDir::new().unwrap();
        let real = touch(dir.path(), "real.mp4", "x");
        symlink(&real, dir.path().join("alias.mp4")).unwrap();

        let canonical = confine(dir.path(), &dir.path().join("alias.mp4")).unwrap();

        assert_eq!(canonical, real.canonicalize().unwrap());
    }

    #[test]
    fn a_symlinked_pool_directory_is_followed() {
        let base = TempDir::new().unwrap();
        let real = base.path().join("real");
        fs::create_dir(&real).unwrap();
        touch(&real, "a.mp4", "x");
        let link = base.path().join("link");
        symlink(&real, &link).unwrap();
        let prober = FakeProber::default();
        prober.set("a.mp4", 5.0);

        let library = ClipLibrary::scan(&[pool("p", &link)], prober);

        assert_eq!(names(library.pool("p").unwrap()), ["a.mp4"]);
        assert_eq!(library.issues().count(), 0);
    }

    #[test]
    fn a_scan_refuses_symlinks_that_leave_the_pool_and_reports_them() {
        let base = TempDir::new().unwrap();
        let dir = base.path().join("clips");
        fs::create_dir(&dir).unwrap();
        touch(&dir, "inside.mp4", "x");
        let outside = touch(base.path(), "outside.mp4", "x");
        symlink(&outside, dir.join("escape.mp4")).unwrap();
        let elsewhere = base.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, dir.join("escape-dir")).unwrap();
        let prober = Arc::new(FakeProber::default());
        prober.set("inside.mp4", 5.0);
        prober.set("outside.mp4", 5.0);

        let library = ClipLibrary::scan(&[pool("p", &dir)], prober.clone());

        let p = library.pool("p").unwrap();
        assert_eq!(names(p), ["inside.mp4"]);
        let refused: Vec<_> = p
            .issues()
            .iter()
            .filter(|i| i.kind == IssueKind::OutsidePool)
            .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(refused, ["escape-dir", "escape.mp4"]);
        // The refused target was never handed to ffprobe.
        assert_eq!(prober.calls(), ["inside.mp4"]);
    }

    #[test]
    fn a_broken_symlink_is_reported_and_does_not_stop_the_scan() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "good.mp4", "x");
        symlink(dir.path().join("gone"), dir.path().join("dangling.mp4")).unwrap();
        let prober = FakeProber::default();
        prober.set("good.mp4", 5.0);

        let library = ClipLibrary::scan(&[pool("p", dir.path())], prober);

        let p = library.pool("p").unwrap();
        assert_eq!(names(p), ["good.mp4"]);
        assert!(matches!(p.issues()[0].kind, IssueKind::Unreadable(_)));
    }

    #[test]
    fn scanned_clips_use_the_canonical_path_of_symlinks_inside_the_pool() {
        let dir = TempDir::new().unwrap();
        let real = touch(dir.path(), "real.mp4", "x");
        symlink(&real, dir.path().join("alias.mp4")).unwrap();
        let prober = FakeProber::default();
        prober.set("real.mp4", 5.0);

        let library = ClipLibrary::scan(&[pool("p", dir.path())], prober);

        let canonical = real.canonicalize().unwrap();
        assert!(
            library
                .pool("p")
                .unwrap()
                .clips()
                .iter()
                .all(|c| c.path == canonical)
        );
    }
}

#[test]
fn the_library_finds_the_pool_that_contains_a_path() {
    let base = TempDir::new().unwrap();
    let ads = base.path().join("ads");
    let idents = base.path().join("idents");
    fs::create_dir(&ads).unwrap();
    fs::create_dir(&idents).unwrap();
    let clip = touch(&idents, "id.mp4", "x");
    let other = touch(base.path(), "other.mp4", "x");
    let library = ClipLibrary::scan(
        &[pool("ads", &ads), pool("idents", &idents)],
        FakeProber::default(),
    );

    let (found, canonical) = library.confine(&clip).unwrap();

    assert_eq!(found.name(), "idents");
    assert_eq!(canonical, clip.canonicalize().unwrap());
    assert!(matches!(library.confine(&other), Err(PathRefused::Outside)));
}

#[test]
fn a_missing_pool_directory_does_not_make_the_library_accept_or_hide_paths() {
    let base = TempDir::new().unwrap();
    let ok = base.path().join("ok");
    fs::create_dir(&ok).unwrap();
    let inside = touch(&ok, "a.mp4", "x");
    let outside = touch(base.path(), "b.mp4", "x");
    let library = ClipLibrary::scan(
        &[pool("lost", &base.path().join("lost")), pool("ok", &ok)],
        FakeProber::default(),
    );

    assert_eq!(library.confine(&inside).unwrap().0.name(), "ok");
    assert!(library.confine(&outside).is_err());
}

#[test]
fn ffprobe_output_is_parsed_as_seconds() {
    assert_eq!(
        parse_seconds("12.500000\n").unwrap(),
        Duration::from_millis(12_500)
    );
    assert_eq!(parse_seconds(" 3 ").unwrap(), secs(3));
    for text in ["", "\n", "N/A", "0", "0.000000", "-4", "NaN"] {
        assert!(
            matches!(parse_seconds(text), Err(ProbeError::NoDuration)),
            "{text:?}"
        );
    }
    for text in ["abc", "1.2.3", "inf"] {
        assert!(
            matches!(parse_seconds(text), Err(ProbeError::Unparseable(_))),
            "{text:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn ffprobe_prober_runs_the_program_without_a_shell_and_enforces_its_timeout() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    // One test for every script, so no other thread forks while a script is
    // being written, which would fail the exec with "text file busy".
    let dir = TempDir::new().unwrap();
    let script = |name: &str, body: &str| {
        let path = touch(dir.path(), name, &format!("#!/bin/sh\n{body}\n"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let clip = dir.path().join("a clip; touch pwned.mp4");
    fs::write(&clip, "x").unwrap();

    // The path arrives as one argument after `-i`, whatever it contains.
    let echo = script(
        "echo-args",
        r#"[ "$7" = "-i" ] && [ "$#" = 8 ] && [ -f "$8" ] && echo 12.25"#,
    );
    let got = FfprobeProber::with_program(&echo).duration(&clip).unwrap();
    assert_eq!(got, Duration::from_millis(12_250));
    assert!(!dir.path().join("pwned.mp4").exists());

    let fails = script("fails", "exit 3");
    assert!(matches!(
        FfprobeProber::with_program(&fails).duration(&clip),
        Err(ProbeError::Failed(Some(3)))
    ));

    let silent = script("silent", "echo N/A");
    assert!(matches!(
        FfprobeProber::with_program(&silent).duration(&clip),
        Err(ProbeError::NoDuration)
    ));

    let hangs = script("hangs", "exec sleep 30");
    let started = Instant::now();
    let result = FfprobeProber::with_program(&hangs)
        .with_timeout(Duration::from_millis(200))
        .duration(&clip);
    assert!(matches!(result, Err(ProbeError::TimedOut(_))));
    assert!(started.elapsed() < Duration::from_secs(10));

    let missing = dir.path().join("no-such-program");
    assert!(matches!(
        FfprobeProber::with_program(&missing).duration(&clip),
        Err(ProbeError::Spawn(_))
    ));
}
