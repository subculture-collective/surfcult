# Documentation

- `idea.md`: product idea and draft specification (codename BROADCAST); technical choices are superseded by the design below
- `superpowers/specs/2026-11-22-surfcult-design.md`: the current design (draft, awaiting approval)
- `reference/stack.md`: tools, versions and commands for this stack
- `reference/storage.md`: the SQLite tables and the migration rule
- `reference/jellyfin.md`: the Jellyfin source: endpoints, the auth header, key sources, the SQLite cache and failed syncs
- `reference/clips.md`: local clip pools, ffprobe durations, what is skipped and why
- `reference/schedule.md`: how the schedule generator orders items, resumes and persists
- `reference/guide.md`: guide cells from the schedule: merging a program's segments, the window, progress
- `reference/api.md`: the read API: endpoints, JSON shapes, the listen address and the bearer token
- `reference/player.md`: the player and MPV: commands, the stand-by card, display filters (the CRT shader and the text glitch), the Jellyfin key, events and the watchdog
- `reference/tuner.md`: the tune-in sequence, clip choice, break stand-in, late-start seek, the stand-by card and how the tuner is driven
- `runbooks/raspberry-pi.md`: reaching and operating the Pi 4B TV box
- `research/2026-11-22-pi-playback-spike.md`: Phase 0 results: decode, Jellyfin tune time, overlay cost
- `../tools/pi-spike/`: the scripts behind those measurements
- `adr/`: architecture decision records; [0001](adr/0001-pi-and-library-findings.md) records what the Pi 4B and the library showed in Phase 0
- `reference/`: stable technical references
- `research/`: exploratory notes and comparisons
- `runbooks/`: repeatable operational procedures

Keep these documents current and link new ones from this page.
