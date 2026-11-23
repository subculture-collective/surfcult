# Clips

Clips are short local video files that fill breaks, cover a tune-in, pad gaps and show slates: ads, station idents, bumpers, previews and emergency slates. They live on the Pi's own storage, not in Jellyfin, so they play without the network.

The code is in `src/clips/`. It reads clips and their lengths. It does not choose or rotate them; the schedule generator and the tuner do that.

## Where clips live

Each `[[interstitial_pool]]` in `channels.toml` names a directory:

```toml
[[interstitial_pool]]
name = "ads-90s"
path = "/var/lib/surfcult/clips/ads-90s"
```

Put the clip files directly in that directory. The scan does not look in subdirectories. Clips in a pool are sorted by file name, so the order is the same on every scan.

## What ffprobe is for

surfcult asks `ffprobe` for each file's length, because the schedule generator fills a break with whole clips and needs exact durations. `ffprobe` is part of FFmpeg and must be on `PATH` on the Pi (`apt install ffmpeg`). It runs as `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 -i <path>`, with the path as one argument and no shell. Each file gets 10 seconds; a file that takes longer is skipped.

Tests do not need `ffprobe`: they use a fake `Prober`.

## What is skipped

A scan never fails because of one bad file. These are left out and reported as issues, with the pool, the path and the reason:

- a file that `ffprobe` cannot read, that has no duration, or whose duration is zero or negative;
- a file that `ffprobe` cannot be run on (for example `ffprobe` is not installed; then every file is reported);
- a file or symlink whose real path is outside the pool directory;
- a broken symlink or an entry that cannot be resolved.

A missing or unreadable pool directory gives an empty pool and one issue.

Subdirectories and files whose name starts with `.` are ignored without a report. Files are not filtered by extension: a stray `notes.txt` is reported as unreadable.

## Paths stay inside the pool

Every path is resolved before it is checked: `..` and symlinks are followed, and the result must be inside the pool's resolved directory. A symlink inside a pool that points elsewhere is refused. `confine` (and `ClipLibrary::confine`, which finds the pool for you) applies the same rule to any path and refuses a path outside every configured directory. Clips carry their resolved path, so what plays is what was checked.

## Rescanning

`ClipLibrary::rescan()` reads the directories again, so added and removed files show up. Durations are cached by resolved path, modified time and size. A file that has not changed is not probed again, which keeps a rescan cheap on the Pi. A file that failed to probe is cached the same way and stays reported until it changes.

## Using the library

```rust
let library = ClipLibrary::scan(&config.pools, FfprobeProber::new());
if let Some(pool) = library.pool("ads-90s") {
    for clip in pool.clips() {
        println!("{} {:?}", clip.path.display(), clip.duration);
    }
    println!("total {:?}", pool.total_duration());
}
for issue in library.issues() {
    eprintln!("skipped: {issue}");
}
```
