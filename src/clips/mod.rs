//! The local clip library: ads, idents, bumpers, tune-in clips, filler and
//! slates that live on the Pi's own storage.
//!
//! [`ClipLibrary::scan`] reads the directories of the configured
//! [`InterstitialPool`](crate::config::InterstitialPool)s and gets each file's
//! length from a [`Prober`]; [`FfprobeProber`] runs `ffprobe`. The scan is not
//! recursive. Each pool lists its clips sorted by file name, so the schedule
//! generator sees the same order on every run.
//!
//! A file that cannot be probed or has no positive duration is left out and
//! reported as an [`Issue`]; a missing pool directory gives an empty pool and
//! an issue. Nothing here panics or stops on a bad file.
//!
//! Paths are confined to the configured directories: a file whose canonical
//! path is outside its pool directory, for example through a symlink, is
//! refused, and [`ClipLibrary::confine`] applies the same rule to any path.
//!
//! [`ClipLibrary::rescan`] picks up added and removed files. Durations are
//! cached by path, modified time and size, so a rescan only probes files that
//! are new or changed.
//!
//! [`ClipLibrary::start`] does not probe in the caller's thread: it takes
//! durations from stored [`ProbeRecord`]s and probes the other files on
//! background threads, and [`ClipLibrary::collect`] adds them to their pools
//! as they finish. [`ClipLibrary::take_changes`] gives the results to store.
//!
//! This module does not choose or rotate clips; the schedule generator and the
//! tuner do that.

mod library;
mod pool;
mod probe;

pub use library::{ClipLibrary, ProbeChanges, ProbeRecord, Probed, Stamp};
pub use pool::{Clip, Issue, IssueKind, PathRefused, Pool};
pub use probe::{FfprobeProber, ProbeError, Prober};

#[cfg(test)]
mod tests;
