# **PTV — Rust-based Retro Television System**

A software specification and design document for a self-hosted, offline-first television channel simulator inspired by DeRose Design's PTV.

The proposed application is designed primarily for Raspberry Pi 4/5, with support for standard Linux desktops. The objective is to recreate the experience of traditional cable television: continuously broadcasting channels, scheduled programming, commercials, station identification, and an electronic program guide.

[PrevueGuide.neocities.org // Prevue Amiga (Esquire)](https://images.openai.com/static-rsc-4/8z8ruLkVAFoXph9QxwKQqUAYP2Tf9z12KigTiXM3R7u5zBlnF7jAbhyBoEAqHvNXasfyY-PAmYVokbAG6lZUwIsc6Sx-8CbceSY0oYUKj8KAXEFyNK9OLowRVPpsFa15uT64X-M_oAH6HFkVDxhMPaCNTL1JUmoC-py2YAmchiA?purpose=inline)

[prevueguide.neocities.org](https://prevueguide.neocities.org/guides/Esquire/1?utm_source=chatgpt.com)

Rust

Linux / Raspberry Pi

Offline-first

Self-hosted

Open source

Proposed project codename: BROADCAST · Specification v0.1 · November 2026

# BROADCAST

## Software Specification & Design Document

**Project:** BROADCAST\
**Version:** 0.1\
**Status:** Initial specification\
**Implementation language:** Rust\
**Primary platform:** Raspberry Pi 4 / Raspberry Pi 5\
**Secondary platforms:** Linux x86_64, Linux ARM64\
**Deployment:** Standalone appliance\
**License proposal:** AGPL-3.0

---

# 1. Product Overview

## 1.1 Vision

BROADCAST is an open-source, self-hosted television broadcasting application that transforms a Raspberry Pi or Linux computer into a simulated television network.

Users organize their media into channels, configure programming schedules, insert advertisements and station identification, and watch continuous broadcasts through a television interface.

The experience should resemble traditional cable television from the 1980s, 1990s, and early 2000s.

The user should not need to choose an individual movie or episode. They should be able to turn on the television, select a channel, and watch whatever is currently broadcasting.

## 1.2 Core Philosophy

- Television should feel like television, not a media library.
- Channels should broadcast continuously.
- Programming should follow a schedule.
- Viewers should join programs in progress.
- The system should work entirely offline.
- The interface should prioritize television navigation over conventional desktop interaction.
- Media should remain locally owned and stored.
- Configuration should be accessible without requiring technical knowledge.

## 1.3 Primary Use Cases

1. Create a channel dedicated to cartoons.
2. Create a channel dedicated to movies.
3. Create a channel dedicated to music videos.
4. Create a channel dedicated to television programming.
5. Create a channel that randomly rotates through selected media.
6. Insert advertisements between programs.
7. Create scheduled blocks, such as Saturday morning cartoons.
8. Display a television guide showing current and upcoming programming.
9. Switch between channels without restarting the broadcast.
10. Operate the entire system using a television remote.

---

# 2. Functional Requirements

## 2.1 Channel Management

Users must be able to create, modify, delete, and organize television channels.

Each channel contains:

- Channel number
- Channel name
- Channel description
- Channel logo
- Channel background image
- Channel theme/accent color
- Media sources
- Programming schedule
- Playback configuration
- Commercial configuration
- Audio configuration
- Enabled/disabled status

Example channel lineup:

| Number | Channel         | Programming            |
| ------ | --------------- | ---------------------- |
| 02     | CARTOON NETWORK | Cartoons and animation |
| 03     | MTV             | Music videos           |
| 04     | MOVIE HOUSE     | Feature films          |
| 05     | SITCOMS         | Television sitcoms     |
| 06     | LATE NIGHT      | Random entertainment   |
| 07     | NEWS 24         | News and documentaries |
| 08     | LOCAL TV        | Mixed programming      |

Channel numbers should be configurable.

## 2.2 Media Library

The application must index locally accessible media.

Supported sources:

- USB drives
- Local filesystem directories
- External hard drives
- Network-mounted directories
- NAS shares mounted by the operating system

Supported formats should include:

- MP4
- MKV
- MOV
- AVI
- MPEG
- TS
- WebM

Audio:

- MP3
- FLAC
- AAC
- WAV
- OGG

The application should use FFprobe to identify media metadata.

Metadata includes:

- Title
- Duration
- Resolution
- Video codec
- Audio codec
- Frame rate
- File size
- Audio tracks
- Subtitle tracks

The media index should detect changes to monitored directories and update automatically.

Users should be able to manually edit metadata.

## 2.3 Programming and Scheduling

The scheduling engine is the central component of BROADCAST.

A channel must support multiple scheduling modes.

### Sequential

Play media in a defined order.

### Random

Select media randomly from an assigned collection.

### Weighted Random

Assign different probabilities to individual media items or collections.

As built, the weights are on sources, not on items. A weighted channel, block or default lists `sources`, each with a `weight` that is a whole number of at least 1 (1 when left out). A slot picks a source in proportion to its weight and then an item from it with the shuffle's cooldown, counted per source. Weights on single items and on commercials (`frequency_weight` below) are not built; give an item its own source, such as `jellyfin_series`, to weight it. See [the schedule reference](reference/schedule.md#weighted).

### Scheduled Blocks

Create programming blocks that run at specified times.

Example:

| Time  | Programming       |
| ----- | ----------------- |
| 06:00 | Saturday Cartoons |
| 09:00 | Classic Animation |
| 12:00 | Family Movies     |
| 14:00 | Sitcom Marathon   |
| 18:00 | Prime Time        |
| 22:00 | Late Night        |

### Time-Synchronized Playback

Playback position must be derived from a channel's broadcast timeline.

The system should calculate the current program and playback position using a persistent broadcast epoch and schedule.

When a viewer changes channels, playback starts at the position appropriate to the current broadcast time.

The system must not restart programs whenever a viewer changes channels.

The scheduling engine must account for:

- Program duration
- Commercial breaks
- Filler content
- Scheduled transitions
- Channel-specific playback rules
- Day-of-week scheduling
- Time zones
- Daylight saving time

## 2.4 Commercials and Interstitials

Users can create commercial libraries and configure insertion rules.

Supported content types:

- Advertisements
- Station identification
- Network bumpers
- Program previews
- Public service announcements
- Emergency broadcast screens
- Sign-on and sign-off screens
- Countdown sequences

Commercial insertion modes:

1. Fixed interval
2. Between programs
3. At predefined timestamps
4. Randomized insertion
5. Scheduled advertising blocks

Users can define commercial breaks by duration.

Example:

A 30-minute television episode may have:

- 12-minute mark: 2-minute break
- 22-minute mark: 2-minute break
- End: 30-second station identification

Commercials should support configurable rotation and frequency limits.

## 2.5 Electronic Program Guide (EPG)

The application must provide a television-style program guide.

The guide should display:

- Channel number
- Channel name
- Channel logo
- Currently playing program
- Upcoming programs
- Program start and end times
- Program descriptions
- Progress indicators

The guide should support:

- Current time indicator
- Horizontal timeline navigation
- Vertical channel navigation
- Program information
- Channel selection
- Configurable guide duration

Default guide view: current time through the next three hours.

The guide should be available both locally and through an optional browser-based administration interface.

## 2.6 Playback

Playback must support:

- Continuous playback
- Channel switching
- Seeking where permitted
- Volume control
- Mute
- Audio track selection
- Subtitle selection
- Aspect ratio handling
- Resolution adaptation
- Playback recovery
- Black-screen transitions
- Station identification overlays

The player should support hardware acceleration where available.

Playback should prioritize direct playback rather than transcoding.

If a media file cannot be played, the application should log the error and advance to the next scheduled item.

---

# 3. Technical Architecture

## 3.1 Architectural Approach

BROADCAST should use a modular Rust architecture.

The application should be organized around independent services communicating through well-defined interfaces and internal events.

A single-process architecture is recommended initially.

This reduces deployment complexity on Raspberry Pi while retaining modularity.

## 3.2 Technology Stack

| Component             | Technology                     |
| --------------------- | ------------------------------ |
| Language              | Rust                           |
| Async runtime         | Tokio                          |
| Application framework | Axum                           |
| Serialization         | Serde                          |
| Database              | SQLite                         |
| Database access       | SQLx                           |
| Media playback        | MPV                            |
| Playback integration  | JSON IPC                       |
| Media metadata        | FFprobe                        |
| Media processing      | FFmpeg                         |
| Filesystem monitoring | notify                         |
| Scheduling            | Custom Rust scheduler + Chrono |
| Frontend              | Leptos or React                |
| Desktop UI            | Web-based kiosk                |
| Configuration         | TOML                           |
| Logging               | tracing                        |
| Error handling        | anyhow + thiserror             |
| CLI                   | Clap                           |
| HTTP client           | Reqwest                        |
| Image processing      | image                          |

### Important Playback Decision

Use MPV as an external playback process controlled through its JSON IPC interface.

Do not implement a video decoder or media playback engine from scratch.

MPV provides mature media playback, hardware acceleration support, subtitle handling, audio selection, and broad codec compatibility.

The Rust application should own scheduling and playback orchestration.

## 3.3 System Architecture

```
┌──────────────────────────────────────────┐
│               PRESENTATION               │
│                                          │
│  Television UI     Admin UI     REST API │
└──────────────────────┬───────────────────┘
                       │
┌──────────────────────▼───────────────────┐
│              APPLICATION CORE            │
│                                          │
│  Channel Manager    Media Library        │
│  Scheduler          EPG Generator        │
│  Playback Manager   Commercial Manager   │
│  Configuration      Event Dispatcher     │
└──────────────────────┬───────────────────┘
                       │
┌──────────────────────▼───────────────────┐
│               INFRASTRUCTURE             │
│                                          │
│  SQLite             Filesystem Watcher   │
│  MPV IPC            FFprobe              │
│  FFmpeg             System Integration   │
└──────────────────────────────────────────┘
```

## 3.4 Core Modules

### broadcast-core

Shared domain models, interfaces, events, and business rules.

### broadcast-media

Media indexing, metadata extraction, filesystem monitoring, and library management.

### broadcast-channel

Channel creation, configuration, channel ordering, and channel identity.

### broadcast-scheduler

Programming schedules, broadcast timelines, commercial insertion, and time synchronization.

### broadcast-player

MPV process management, JSON IPC, playback state, and error recovery.

### broadcast-epg

Program guide generation, timeline calculations, and EPG data export.

### broadcast-server

HTTP API, WebSocket events, authentication for administration, and remote configuration.

### broadcast-ui

Television interface, program guide, overlays, and administrative views.

### broadcast-cli

Initial setup, diagnostics, configuration validation, and maintenance commands.

---

# 4. Data Model

SQLite is the authoritative data store.

## 4.1 Primary Entities

### Channel

- id: UUID
- number: integer
- name: string
- description: string
- logo_path: optional string
- enabled: boolean
- playback_mode: enum
- created_at: timestamp
- updated_at: timestamp

### MediaItem

- id: UUID
- file_path: string
- title: string
- description: optional string
- duration_ms: integer
- media_type: enum
- width: optional integer
- height: optional integer
- codec: optional string
- audio_tracks: JSON
- subtitle_tracks: JSON
- file_hash: optional string
- indexed_at: timestamp

### ChannelMedia

- channel_id: UUID
- media_id: UUID
- position: optional integer
- weight: optional float (not built per item; as built, a whole number per source in `sources`; see Weighted Random)
- enabled: boolean

### Schedule

- id: UUID
- channel_id: UUID
- name: string
- schedule_type: enum
- start_time: optional timestamp
- recurrence: optional JSON
- priority: integer

### ScheduleItem

- id: UUID
- schedule_id: UUID
- media_id: UUID
- start_offset_ms: optional integer
- duration_override_ms: optional integer
- position: integer

### Commercial

- id: UUID
- media_id: UUID
- category: string
- duration_ms: integer
- enabled: boolean
- frequency_weight: float (not built; see Weighted Random)

### BroadcastEvent

- id: UUID
- channel_id: UUID
- media_id: UUID
- scheduled_start: timestamp
- scheduled_end: timestamp
- actual_start: optional timestamp
- actual_end: optional timestamp
- event_type: enum

## 4.2 Additional Tables

- media_sources
- media_collections
- channel_collections
- commercial_breaks
- channel_settings
- system_settings
- playback_history
- scan_jobs
- schedule_templates

Use database migrations from the beginning.

---

# 5. Broadcast Scheduling Engine

## 5.1 Broadcast Timeline

Every channel should maintain its own logical broadcast timeline.

The timeline must be deterministic.

A channel should produce the same schedule regardless of whether a television is currently viewing it.

Example:

Channel 03:

| Start | End   | Content          |
| ----- | ----- | ---------------- |
| 12:00 | 12:03 | Music Video A    |
| 12:03 | 12:07 | Music Video B    |
| 12:07 | 12:09 | Commercial Break |
| 12:09 | 12:13 | Music Video C    |
| 12:13 | 12:17 | Music Video D    |

At 12:11, a viewer tuning into Channel 03 should see Music Video C approximately two minutes into playback.

## 5.2 Timeline Calculation

The scheduling engine should maintain a broadcast epoch and a generated playlist.

For a fixed-duration repeating playlist:

```
elapsed = current_time - broadcast_epoch

position = elapsed % playlist_duration

current_item = locate_item(position)

playback_offset = position - current_item.start_offset
```

For schedules containing different programming blocks, use an absolute timeline of generated broadcast events rather than assuming one repeating playlist.

## 5.3 Schedule Persistence

Broadcast schedules must survive:

- Application restarts
- System restarts
- Temporary media disconnections
- Power interruptions

Persist the schedule seed and broadcast epoch.

Randomized programming must be deterministic within a generated schedule window.

The scheduler should generate a rolling schedule horizon, such as the next seven days.

## 5.4 Schedule Conflicts

The scheduler must define precedence rules:

1. Explicitly scheduled programming
2. Scheduled programming blocks
3. Channel default programming
4. Filler programming
5. Station identification

If media is unavailable, the scheduler should select valid replacement content and record the substitution.

---

# 6. User Interface Design

## 6.1 Design Direction

Visual inspiration:

- 1990s cable television
- CRT televisions
- VHS interfaces
- Analog broadcast graphics
- Early digital cable guides
- Television station branding

The interface should be designed for a 10-foot viewing distance.

Use large typography, strong contrast, recognizable channel identities, and minimal text.

Avoid conventional desktop application aesthetics.

## 6.2 Main Television Screen

The main screen should show:

- Full-screen video
- Channel number
- Channel name
- Current program title
- Broadcast progress
- Current time
- Optional station logo

Overlays should disappear automatically after a short timeout.

## 6.3 Electronic Program Guide

The EPG should resemble a traditional cable television guide.

Layout:

- Left column: channel number, logo, name
- Main area: horizontal program timeline
- Top row: time markers
- Highlighted cell: currently playing program
- Bottom area: selected program information

The guide should be navigable using directional controls.

## 6.4 Channel Information Overlay

On channel change:

Display:

- Channel number
- Channel name
- Channel logo
- Program title
- Program description
- Current time
- Remaining duration

## 6.5 Administration Interface

The administration interface should provide:

**Dashboard**

- Active channels
- Current programming
- Media library status
- Storage usage
- System health

**Channels**

- Create and edit channels
- Upload logos
- Configure playback
- Assign media collections

**Media Library**

- Browse indexed files
- Search
- Edit metadata
- Manage collections
- Identify unsupported files

**Programming**

- Schedule editor
- Timeline editor
- Recurring programming
- Commercial insertion
- Preview schedule

**System**

- Storage management
- Playback diagnostics
- Logging
- Configuration
- Software updates

---

# 7. Remote Control Support

The application must work without a keyboard or mouse.

Primary navigation:

| Input         | Action                     |
| ------------- | -------------------------- |
| Up / Down     | Channel selection          |
| Left / Right  | Volume or guide navigation |
| OK / Enter    | Open selection             |
| Back / Escape | Close overlay              |
| Menu          | Open television menu       |
| Guide         | Open EPG                   |
| Info          | Program information        |
| 0–9           | Direct channel selection   |
| CH+ / CH-     | Change channels            |
| Mute          | Toggle mute                |
| Play/Pause    | Playback control           |

Support Linux input devices, USB remotes, keyboard input, and compatible HDMI-CEC devices where supported.

---

# 8. Hardware Requirements

## 8.1 Minimum Hardware

- Raspberry Pi 4
- 2GB RAM
- microSD or SSD storage
- HDMI television
- USB media storage

## 8.2 Recommended Hardware

- Raspberry Pi 5
- 4GB RAM
- USB 3.0 SSD
- Active cooling
- Reliable power supply
- HDMI television

## 8.3 Performance Requirements

- Target 1080p playback at 30/60 FPS where supported by the source and hardware.
- Minimize unnecessary transcoding.
- Channel switching should generally complete within three seconds.
- EPG should load within one second under normal conditions.
- Media indexing should operate in the background.
- Playback should recover automatically after transient errors.

Performance targets must be validated on actual hardware.

---

# 9. Reliability and Recovery

The application should behave like an appliance.

Requirements:

- Automatic startup on boot.
- Automatic playback process recovery.
- Persistent channel configuration.
- Persistent broadcast timelines.
- Graceful handling of disconnected drives.
- Graceful handling of missing media.
- Structured logs.
- Hardware temperature monitoring.
- Storage monitoring.
- Safe shutdown support.

Use systemd for service management.

The playback process must be independently restartable without losing scheduler state.

---

# 10. Networking and Integrations

Networking should be optional.

The application should remain fully functional without an internet connection.

Optional functionality:

- Browser-based administration.
- Remote control through REST API.
- WebSocket playback updates.
- M3U playlist export.
- XMLTV EPG export.
- External IPTV client integration.
- Home Assistant integration.

The initial version should not depend on Plex, Jellyfin, Emby, or an external streaming service.

BROADCAST should own its own media index and scheduling engine.

---

# 11. Security

- No required external network access.
- No telemetry by default.
- Administrative interface should be local-only by default.
- Optional authentication for remote administration.
- Filesystem access should be restricted to configured media directories.
- No arbitrary shell command execution through the API.
- Validate imported playlists and configuration files.
- Never automatically delete user media.

---

# 12. Development Roadmap

## Phase 1: Playback Foundation

- Rust application skeleton.
- SQLite database.
- MPV process management.
- Media indexing.
- Metadata extraction.
- Basic television output.
- Continuous sequential playback.

**Milestone:** A Raspberry Pi boots directly into a television that continuously plays a media directory.

## Phase 2: Channels

- Channel CRUD.
- Multiple channel playlists.
- Channel switching.
- Independent broadcast timelines.
- Random playback.
- Channel persistence.

**Milestone:** Multiple channels broadcast continuously and can be switched between.

## Phase 3: Scheduling

- Scheduling engine.
- Scheduled programming blocks.
- Deterministic randomization.
- Commercial insertion.
- Filler programming.
- Broadcast event persistence.

**Milestone:** Channels operate according to a defined programming schedule.

## Phase 4: Television Interface

- Main television screen.
- Channel information overlay.
- Electronic program guide.
- Remote control.
- Channel branding.
- Navigation system.

**Milestone:** The application can be operated entirely from a television remote.

## Phase 5: Administration

- Browser-based management.
- Media collection management.
- Schedule editor.
- Channel configuration.
- System dashboard.

**Milestone:** Users can configure their television network without editing configuration files.

## Phase 6: Advanced Features

- Music video channels.
- Station identification.
- Emergency broadcast simulation.
- Weather channel.
- Sign-on and sign-off.
- XMLTV export.
- M3U export.
- HDMI-CEC support.
- Multi-device playback.

**Milestone:** A complete self-hosted television system.

---

# 13. Testing Strategy

## Unit Tests

- Timeline calculations.
- Playback position calculations.
- Schedule generation.
- Commercial insertion.
- Randomized scheduling.
- Channel configuration.
- EPG generation.

## Integration Tests

- MPV process lifecycle.
- IPC communication.
- Filesystem indexing.
- SQLite persistence.
- Media disconnection.
- Scheduler restart recovery.

## Hardware Tests

- Raspberry Pi 4 playback.
- Raspberry Pi 5 playback.
- Hardware acceleration.
- HDMI audio.
- Remote input.
- Long-duration playback.
- Power interruption recovery.

## Acceptance Tests

1. Create three channels.
2. Assign different media collections.
3. Start broadcasting.
4. Switch between channels.
5. Verify that programs resume at their correct broadcast positions.
6. Insert commercial breaks.
7. Restart the application.
8. Verify broadcast continuity.
9. Disconnect a media drive.
10. Verify graceful recovery.

---

# 14. Initial Repository Structure

```
broadcast/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── rust-toolchain.toml
│
├── crates/
│   ├── broadcast-core/
│   ├── broadcast-media/
│   ├── broadcast-channel/
│   ├── broadcast-scheduler/
│   ├── broadcast-player/
│   ├── broadcast-epg/
│   ├── broadcast-server/
│   ├── broadcast-cli/
│   └── broadcast-config/
│
├── frontend/
│   └── television-ui/
│
├── migrations/
├── config/
├── assets/
│   ├── logos/
│   ├── themes/
│   ├── fonts/
│   └── overlays/
│
├── packaging/
│   ├── systemd/
│   ├── raspberry-pi/
│   └── desktop/
│
├── docs/
│   ├── architecture.md
│   ├── scheduling.md
│   ├── api.md
│   ├── database.md
│   └── development.md
│
└── tests/
```

---

# 15. Open Architectural Decisions

The following decisions should be resolved before implementation.

1. **Playback architecture:** One MPV process per active channel, or one shared process that switches channels? The initial recommendation is one active playback process, with independent scheduler state for every channel.
2. **Frontend:** Leptos/WASM, React, or a native Qt/QML application? The recommendation is a web-based UI served locally, running in kiosk mode.
3. **Scheduling:** Generate a rolling seven-day schedule or calculate the timeline dynamically? The recommendation is deterministic rolling schedule generation with persisted events.
4. **Multiple simultaneous outputs:** Should the initial application support multiple televisions or just one output? The recommendation is one local output initially.
5. **Analog output:** Should composite video be supported directly, or through Raspberry Pi hardware/configuration? The recommendation is HDMI first, composite compatibility second.
6. **Live sources:** Should users be able to mix local media with live IPTV streams? Defer this to a later release.
7. **Configuration:** Should users configure everything through the GUI, or should advanced TOML configuration remain available? Support both.

---

# 16. Definition of Done: MVP

The MVP is complete when:

- The application installs on Raspberry Pi OS.
- The system boots directly into BROADCAST.
- Users can index local media.
- Users can create at least two channels.
- Channels continuously broadcast independently.
- Channel switching joins the current broadcast position.
- Programming schedules persist through restarts.
- The application supports commercial insertion.
- A basic EPG displays current and upcoming programming.
- A television remote can control the application.
- The application works without internet access.

**Primary objective:** Create an open-source alternative to proprietary retro television appliances, with a modular architecture that allows the community to extend it into a full television broadcasting platform.

## **Design direction**

I would give the application a distinct visual identity rather than copying PTV's interface. The following are the three visual directions I'd explore.

[User Interface UI Design by Jan Elmer](https://images.openai.com/static-rsc-4/kcRgd7H4S9qyYRGJ0YqP0PrMxbjY3-e-jlHlvV8sPyjgAoSJTp0DLRndxrsPtkuBmjEMHainFeZnfECoebqtjJz4FLkY3H9wEryIwLlwz97L3dzeVRGLjscKs9lCBQD_G4wF9yNXgMp5g6e-Ju-Tk-RCZ5udDssndZjZFTocAQo?purpose=inline)

**A. 90s Cable**

Blue program grids, channel logos, pixel fonts, familiar cable television aesthetic.

[MTV 90s: 2022 Idents & Presentation | Idents & Presentation](https://images.openai.com/static-rsc-4/rnADfIShj_B349PwrdoD80yLnRd3FmFZnGV8UVUDoJ1WiWYIYuBz9paloPM31tHvaGM0HVG4m3u3qcogJWPIrFv4G7zsJ0bsqT51Yqipgt9A6bFdIqrPuQWzpywKevBkWwPvyPH_kdqCpUoG4Mnm6q17NQuGScnesIIS3DC_BZU?purpose=inline)

**B. Analog Underground**

VHS, MTV, underground television, broadcast glitches, bold typography.

[テレビ局の編集者になって英国民のイデオロギーを操作する「Not For Broadcast」VRゲーム版が発売決定 - MoguLive](https://images.openai.com/static-rsc-4/MPlZh7iEtXrh6CJE0SusO-j8h0LmlijfUugN6hajlNA7OVH9qo-022X3XU9AH3Q2r0SLV5jT_AZPiCXEJOMGnJQatO-iDCdYtcWCSoocCVNMRssMWAe_fwdtdMcxu4tJ1072SA2ry63075WTM-FyrJidlVcltLc2exh5J8J3OgM?purpose=inline)

**C. Broadcast Terminal**

Station IDs, technical overlays, broadcast clocks, channel diagnostics.

[Blippo+  for Nintendo Switch - Nintendo Official Site](https://images.openai.com/static-rsc-4/5g6zr3eWbCw3DVvkTOdhZAmgSPdrkUFDXbytn0lIlPrDfJoY_kBo69Q0wLqwYvI6CfcnYQxndFQVjrfIRfIKpiE-Xci9eFP0MKPXUr_SmsD-8FJeJiY62iG8aySuPILMb35EEgDSbYeXwXh9oZ1iikXonTISxV2yGEPC_TT7mRc?purpose=inline)

**D. Retro Futurism**

Dark interfaces, CRT effects, neon accents, nostalgic digital television.

## **Important architectural recommendations**

Three decisions I'd make early:

1. **Keep the scheduler independent of the player.** Every channel should have a logical broadcast timeline, even when nobody is watching it. This is fundamental to the experience.
2. **Use MPV rather than writing a Rust video player.** Rust should own the application logic, while MPV handles decoding and playback.
3. **Design the EPG as a first-class feature.** It should not be an afterthought or merely a list of media files.

## **Suggested initial development order**

1. **Rust workspace and application core**

   Database, configuration, logging, domain models.
2. **Media indexing**

   Scan directories, extract metadata, organize collections.
3. **Broadcast scheduler**

   Deterministic timelines, programming blocks, commercials.
4. **MPV playback integration**

   Continuous playback, synchronization, recovery.
5. **Television interface**

   Channel switching, overlays, EPG, remote navigation.

**One design decision worth settling before implementation:** I recommend treating this as a standalone television appliance first, with an optional web administration interface. That avoids turning the project into another media server with a television skin.

It would be relatively straightforward to add IPTV support if we design the architecture for it from the beginning. I would actually recommend supporting both local HDMI playback and IPTV as two output modes of the same broadcasting system.

The important distinction is that **the scheduler should not care where a channel is being watched.**

A channel broadcasts independently of its viewers. HDMI, IPTV clients, and potentially multiple televisions simply consume that broadcast.

## **1. Recommended architecture**

**BROADCAST CORE**

Media library · Channels · Scheduling · Commercials · EPG

**Broadcast Output Layer**

**Local HDMI**

MPV playback

**IPTV**

HTTP streaming

**Television**

HDMI / Raspberry Pi

**IPTV Clients**

VLC · Kodi · TiviMate · Jellyfin clients

**EPG / M3U / XMLTV**

Shared channel lineup and program information

## **2. How IPTV would work**

I'd implement three major components.

### **A. Streaming server**

The server generates an IPTV stream for each channel.

Recommended technologies:

| Component            | Technology |
| -------------------- | ---------- |
| HTTP server          | Axum       |
| Stream packaging     | MPEG-TS    |
| Video processing     | FFmpeg     |
| Stream orchestration | Rust       |
| Playlist format      | M3U        |
| Program guide        | XMLTV      |
| Transport            | HTTP       |

Example channel URLs:

```
http://broadcast.local:8080/live/1
http://broadcast.local:8080/live/2
http://broadcast.local:8080/live/3
```

Each URL represents a continuous television broadcast.

### **B. M3U playlist**

BROADCAST automatically generates an IPTV playlist.

Example:

```
#EXTM3U
#EXTINF:-1 tvg-id="cartoons" tvg-name="Cartoons" group-title="Entertainment",02 CARTOONS
http://broadcast.local:8080/live/2

#EXTINF:-1 tvg-id="movies" tvg-name="Movies" group-title="Movies",03 MOVIES
http://broadcast.local:8080/live/3

#EXTINF:-1 tvg-id="music" tvg-name="Music Videos" group-title="Music",04 MUSIC
http://broadcast.local:8080/live/4
```

You could load this playlist into VLC, Kodi, TiviMate, or other compatible IPTV applications.

### **C. XMLTV program guide**

The scheduler already knows what each channel is playing.

It can export that information into XMLTV:

- Current program
- Upcoming program
- Start and end times
- Program descriptions
- Channel logos
- Categories

This lets compatible IPTV applications display a traditional television guide.

## **3. The biggest technical decision: stream generation**

There are three ways to approach this.

| Approach                               | Complexity    | Recommendation |
| -------------------------------------- | ------------- | -------------- |
| Generate a stream per viewer           | Low initially | Avoid          |
| Generate one shared stream per channel | Medium        | Best           |
| Generate segmented HLS per channel     | Medium-high   | Optional later |

**I'd use one shared MPEG-TS stream per active channel.**

This avoids encoding the same channel separately for every viewer.

For example:

- 5 viewers watching Channel 2 = one stream.
- 10 viewers watching Channel 3 = one stream.
- No viewers watching Channel 4 = no stream generation necessary.

The scheduler still maintains all channel timelines, regardless of whether streams are active.

When a viewer connects, the server calculates the current broadcast position and joins the stream.

One important implementation detail: MPEG-TS is a transport format, not a complete broadcast engine. We'll need to handle segment boundaries, keyframes, timestamp continuity, buffering, and client reconnections.

## **4. Hardware implications**

Your Raspberry Pi hardware will influence how ambitious we can be.

| Hardware  | Practical role                        |
| --------- | ------------------------------------- |
| Pi 4, 2GB | A few direct-play channels            |
| Pi 4, 4GB | Several lightweight channels          |
| Pi 5, 4GB | Better streaming and orchestration    |
| Pi 5, 8GB | More headroom for concurrent channels |

These are rough deployment targets, not guaranteed channel capacities. Codec, resolution, bitrate, transcoding, and network conditions matter considerably.

The key is to avoid transcoding unnecessarily.

If the source is already compatible, we should remux it into MPEG-TS without re-encoding whenever possible.

For incompatible sources, we can optionally use FFmpeg to transcode.

## **5. Changes I'd make to the software specification**

I'd add the following modules:

| New module         | Responsibility                          |
| ------------------ | --------------------------------------- |
| `broadcast-stream` | Stream generation and lifecycle         |
| `broadcast-iptv`   | IPTV playlists and client compatibility |
| `broadcast-hls`    | HLS packaging and delivery              |
| `broadcast-output` | Shared output abstraction               |
| `broadcast-epg`    | XMLTV generation and serving            |

And introduce these features:

- Multiple simultaneous viewers.
- Shared channel streams.
- On-demand stream activation.
- M3U playlist generation.
- XMLTV program guide generation.
- HLS output.
- MPEG-TS output.
- Stream health monitoring.
- Client connection tracking.
- Network bandwidth monitoring.
- Configurable stream quality.

## **6. Future possibilities**

[我们的下一代，还有机会和家人一起看电视吗？ - 红蚂蚁](https://images.openai.com/static-rsc-4/6qX4mH60L17-nuTaFcw92yADeq6WYy3eDxxcFq-UO4YeMlrWmUMKItFyJUoW7gBkARxAhJR1PulOj4Mw8Rxy5En_q0vI9heuee3X83iY3OOt-v9aK29RC3O-t6Zfm6gp5kXyk1VFAHXwORkqB0AE25n7xrco0RoYJLiuxty2NCk?purpose=inline)

**Local television**

A Raspberry Pi connected directly to a CRT or modern television.

[The Ultimate Guide to Purchasing IPTV  – plume.com](https://images.openai.com/static-rsc-4/jYIOP0iKXbocbOi5rfp4Q_-crAZ3qx-yD6dsCylZhY6NE-OviaCB-vFWWW-rbYmmx7lFq33q7YG_RzW9OCRvVjEYM_LoMbBsgQrqLtgByoW15nGgZDHSFnr0cxup4MlXklPaHPR7W2KSUWNb-nDPZrEaGrjf8QgqFH6R8b0X_Es?purpose=inline)

**Whole-home IPTV**

Multiple televisions watching different channels.

[10 years of Sky+ - the EPGs | TechRadar](https://images.openai.com/static-rsc-4/yhReXbDBmPOZ7SOVCJV-bhMw6wdDkO5-1O8J5TFEyAMpQAG-eVpQ5gY6bapXroQP-8yqzyEsVw9_P6EM2VZhkEUeoPLisn9FLJzIiTw3kyeItJIMpZNA0I_YFp-xLbcm0PV1ua3MNL5ZOT7TbOVbP-Vue9h3s_7qvrL80kQ_k3I?purpose=inline)

**External IPTV clients**

VLC, Kodi, and compatible IPTV applications.

[41 vintage photos that show 'Saturday Night Live' in its '70s heyday](https://images.openai.com/static-rsc-4/XMfN5Z9PgyBuzKoQtYL7MlQEdIVWBZGCiNtI2JMIFjMI7fLbij-hvNNwdoOdXrcDu1T4I7h-odncDC6lBJmMmPLEJc3pwlcQMXsQydqdfXNtos-ywaRrIe2yCxI5OAHOeD98eW7DR2iAUCg2dtKDhE_OCB4po3UgPW0WalBcNfg?purpose=inline)

**Network broadcasting**

Potentially stream your own channels to other networks or remote viewers.

## **7. My recommended scope**

I would make both output modes first-class features, not separate applications.

Recommended architecture

**BROADCAST v1:**

1. Rust scheduling and channel engine.
2. MPV-based local HDMI playback.
3. Shared per-channel IPTV streaming.
4. M3U playlist generation.
5. XMLTV EPG generation.
6. HTTP MPEG-TS output.
7. Optional HLS output.
8. Web-based administration.

I would also make one distinction in the design: the local HDMI output should be treated as an IPTV client of the same broadcast engine, rather than having its own independent playback scheduler. That prevents the HDMI and IPTV versions of a channel from drifting apart.

**One question about the intended deployment:** Should the Raspberry Pi primarily act as a server broadcasting channels to your home network, with HDMI as an optional local viewer, or should it be a standalone television box that also happens to expose IPTV streams to the network?

I'd favor the first architecture because it gives you much more flexibility without significantly increasing the initial development burden.
