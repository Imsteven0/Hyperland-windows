# Plugin examples

Copy a folder into `%USERPROFILE%\.config\caelestia\plugins\` and press
**Settings › Plugins › Reload**.

| Field      | Meaning                                                                 | Default     |
|------------|-------------------------------------------------------------------------|-------------|
| `name`     | Title shown in Settings and on the widget (required)                    | —           |
| `icon`     | [Material Symbols](https://fonts.google.com/icons) name                 | `extension` |
| `exec`     | Script in the folder (`.ps1 .py .js .bat .cmd .exe`), a command line run by `cmd /c`, or an argv array | none |
| `text`     | Fixed text when there is no `exec`                                      | `""`        |
| `interval` | Milliseconds between runs (minimum 1000); `0` runs once                 | `10000`     |
| `timeout`  | Milliseconds before a run is killed                                     | `10000`     |
| `onClick`  | Same forms as `exec`, or a URI such as `ms-settings:sound`              | none        |
| `location` | `dashboard` (overview tab) or `bar`                                     | `dashboard` |

The first line of the command's output is the widget text and the rest its
tooltip. Output a JSON object instead for more control, like waybar:

```json
{ "text": "42%", "tooltip": "details", "icon": "memory", "class": "warning", "percentage": 42 }
```

`class` may be `good`, `warning` or `critical` (tints the widget);
`percentage` (0–100) draws a progress bar on dashboard widgets.

Plugins run code on your machine: only install plugins you trust. Dots
imported from GitHub never include plugins.
