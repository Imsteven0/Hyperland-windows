//! Default `shell.json`, mirroring Caelestia's schema so dots can be shared
//! between the Linux and Windows shells. Windows-only keys live under
//! `windows.*`; everything else keeps Caelestia's names and meaning.

use serde_json::{Value, json};

pub fn default_config() -> Value {
    json!({
        "appearance": {
            "anim": { "durations": { "scale": 1.0 } },
            "font": {
                "family": {
                    "clock": "Rubik",
                    "material": "Material Symbols Rounded",
                    "mono": "Cascadia Mono",
                    "sans": "Rubik"
                },
                "size": { "scale": 1.0 }
            },
            "padding": { "scale": 1.0 },
            "rounding": { "scale": 1.0 },
            "spacing": { "scale": 1.0 },
            "transparency": { "enabled": false, "base": 0.85, "layers": 0.4 }
        },
        "general": {
            "language": "auto",
            "apps": {
                "terminal": ["wt"],
                "audio": ["ms-settings:sound"],
                "explorer": ["explorer"]
            }
        },
        "background": {
            "enabled": true
        },
        "bar": {
            "persistent": true,
            "showOnHover": true,
            "entries": [
                { "id": "logo", "enabled": true },
                { "id": "workspaces", "enabled": true },
                { "id": "spacer", "enabled": true },
                { "id": "activeWindow", "enabled": true },
                { "id": "spacer", "enabled": true },
                { "id": "tray", "enabled": true },
                { "id": "clock", "enabled": true },
                { "id": "statusIcons", "enabled": true },
                { "id": "power", "enabled": true }
            ],
            "sizes": { "innerWidth": 40 },
            "status": {
                "showAudio": true,
                "showBattery": true,
                "showBluetooth": true,
                "showNetwork": true,
                "showMicrophone": true
            },
            "workspaces": { "shown": 5, "activeIndicator": true, "activeTrail": true, "showWindows": false }
        },
        "border": { "rounding": 25, "thickness": 10 },
        "dashboard": {
            "enabled": true,
            "showOnHover": true,
            "mediaUpdateInterval": 1000,
            "resourceUpdateInterval": 1000
        },
        "launcher": {
            "actionPrefix": ">",
            "calcPrefix": "=",
            "emojiPrefix": ":",
            "clipboardPrefix": ";",
            "clipboardHistory": true,
            "clipboardMaxItems": 50,
            "clipboardPaste": false,
            "maxShown": 7,
            "maxWallpapers": 9,
            "enableDangerousActions": false,
            "hiddenApps": []
        },
        "paths": {
            "wallpaperDir": "~/Pictures/Wallpapers",
            "wallpaperRepo": "",
            "recordingsDir": "~/Videos/Recordings",
            "screenshotsDir": "~/Pictures/Screenshots"
        },
        "services": {
            "audioIncrement": 0.1,
            "maxVolume": 1.0,
            "weatherLocation": "",
            "useFahrenheit": false,
            "useTwelveHourClock": true,
            "smartScheme": true
        },
        "plugins": {
            "disabled": []
        },
        "session": {
            "enabled": true
        },
        "utilities": { "enabled": true, "showOnHover": true },
        "windows": {
            "autostart": true,
            "blur": true,
            "performance": {
                "mode": "auto",
                "onBattery": true,
                "onFullscreen": true
            },
            "komorebi": {
                "enabled": false,
                "autostart": true,
                "borders": true,
                "borderWidth": 3,
                "borderOffset": 0,
                "workspacePadding": 10,
                "containerPadding": 10,
                "animations": true,
                "layeredApps": ["*", "claude.exe"],
                "binds": {}
            },
            "shortcuts": {
                "launcher": "Alt+Space",
                "dashboard": "Ctrl+Alt+KeyD",
                "utilities": "Ctrl+Alt+KeyU",
                "session": "Ctrl+Alt+KeyP",
                "settings": "Ctrl+Alt+KeyS"
            }
        }
    })
}
