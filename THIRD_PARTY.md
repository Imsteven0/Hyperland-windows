# Third-party notices

Caelestia for Windows is licensed under the [GNU GPL v3.0](LICENSE). It
recreates the look and the `shell.json` format of
[Caelestia](https://github.com/caelestia-dots/shell) (also GPL-3.0) and bundles
or links the following works, each under its own license.

## Fonts (bundled in `assets/fonts`)

| Font | Copyright | License | Source |
|------|-----------|---------|--------|
| Material Symbols Rounded (`MaterialSymbolsRounded.ttf`) | Copyright Google LLC | [Apache License 2.0](licenses/Apache-2.0.txt) | [google/material-design-icons](https://github.com/google/material-design-icons) |
| Rubik (`Rubik.ttf`) | Copyright 2015 The Rubik Project Authors | [SIL Open Font License 1.1](licenses/OFL-1.1.txt) | [googlefonts/rubik](https://github.com/googlefonts/rubik) |

Both fonts are distributed unmodified. Under the OFL, Rubik may be bundled
and redistributed with this software but not sold on its own; its license
text travels with it in `licenses/OFL-1.1.txt`.

## Rust crates (compiled into `caelestia.exe`)

Direct dependencies; their own dependencies are listed in `Cargo.lock`
(`cargo tree` shows the full graph, and every crate's license is on crates.io).

| Crate | License |
|-------|---------|
| [slint](https://crates.io/crates/slint) (UI toolkit, used under its GPL-3.0 option) | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| [nucleo-matcher](https://crates.io/crates/nucleo-matcher) | MPL-2.0 |
| [emojis](https://crates.io/crates/emojis) (includes Unicode emoji data) | (MIT OR Apache-2.0) AND Unicode-3.0 |
| [rfd](https://crates.io/crates/rfd) | MIT |
| [sysinfo](https://crates.io/crates/sysinfo) | MIT |
| [raw-window-handle](https://crates.io/crates/raw-window-handle) | MIT OR Apache-2.0 OR Zlib |
| [anyhow](https://crates.io/crates/anyhow), [chrono](https://crates.io/crates/chrono), [crossbeam-channel](https://crates.io/crates/crossbeam-channel), [dirs](https://crates.io/crates/dirs), [env_logger](https://crates.io/crates/env_logger), [image](https://crates.io/crates/image), [log](https://crates.io/crates/log), [material-colors](https://crates.io/crates/material-colors), [notify-debouncer-mini](https://crates.io/crates/notify-debouncer-mini), [serde](https://crates.io/crates/serde), [serde_json](https://crates.io/crates/serde_json), [ureq](https://crates.io/crates/ureq), [windows](https://crates.io/crates/windows), [windows-collections](https://crates.io/crates/windows-collections), [wmi](https://crates.io/crates/wmi) | MIT OR Apache-2.0 |

## Services used at runtime

- Weather and place search: [Open-Meteo](https://open-meteo.com/) (CC BY 4.0 data).
- Automatic location (when no weather location is set): [ipinfo.io](https://ipinfo.io/).
- Wallpaper and dots downloads, update checks: the GitHub API and
  `raw.githubusercontent.com`, only when you use those features.
- Optional tiling: [komorebi](https://github.com/LGUG2Z/komorebi) is a separate
  program you install yourself; it has its own license (free for personal use).
