# Taskbar Media Viewer

Taskbar Media Viewer is a small Rust app that displays your active media info directly on top of the Windows taskbar.

It shows track details and gives you quick playback controls (previous, play/pause, next) for the media session that is most relevant right now.

## Why this exists

When multiple apps can play media at the same time (Spotify, browser tabs, VLC, etc.), Windows media controls are not always focused on the app you actually care about and aren't shown at all times

This app solves that by choosing a session based on priority rules and playback state, then exposing controls in a lightweight overlay thats always shown in the taskbar.

## Features

- Transparent, borderless overlay anchored to the taskbar area
- Playback controls for the selected session:
  - Previous track
  - Play/Pause
  - Next track
- Session prioritization based on:
  - Playback state (`Playing` > `Paused` > `Stopped`)
  - App preference score (customizable)
  - Whether track metadata is available
- Auto passthrough behaviour:
  - Clickable only over controls
  - Mouse passthrough elsewhere
- Hides itself while another app is fullscreen
- Topmost tool window behaviour so it stays out of Alt+Tab and taskbar entries

## Stack

- Rust
- `eframe` / `egui` for UI
- `windows` crate for Win32 + media control APIs
- `nowhear` for media session discovery

## Requirements

- Windows (the app exits on non-Windows platforms)
- Rust toolchain (stable)

## Getting started

```bash
cargo run
```

For a release build:

```bash
cargo run --release
```

## Configuration

Current configuration is initialized in `src/main.rs`.

You can tweak:

- `priority_list`: app match strings and scores
- `size`: overlay width/height
- `position`: overlay screen coordinates

Example from the project:

```rust
let mut priority_list: Vec<(String, i32)> = Vec::new();
priority_list.push(("spotify".to_string(), 40));
priority_list.push(("vlc".to_string(), 35));
priority_list.push(("music".to_string(), 30));
priority_list.push(("chrome".to_string(), 25));
priority_list.push(("msedge".to_string(), 25));
let size = [800.0, 46.0];
let position = [0.0, 1033.0];
```

The app IDs are matched with a case-insensitive `contains` check, so partial identifiers are fine.

## How prioritization works

Each media session gets a score:

- Playback state contributes the biggest weight
- Preferred app match adds a secondary weight
- Sessions with track metadata get a small bonus

The highest score wins and is shown in the overlay.

## Project structure

- `src/main.rs` - startup and initial config
- `src/config.rs` - global app config (`OnceLock`)
- `src/media.rs` - session discovery, prioritization, transport actions
- `src/taskbar_gui.rs` - overlay window behaviour and UI rendering

## Contributing

PRs and issues are welcome.

If you want to improve this project, useful next steps include dynamic taskbar detection, external config support, and multi-monitor behaviour.
