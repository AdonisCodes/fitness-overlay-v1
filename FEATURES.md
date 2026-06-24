# Features

## Desktop editor (`./fitnessoverlay` with no args)

- [x] Visual pre-export preview with video frame + burned overlay
- [x] Timeline scrubber / seeking within the activity overlap window
- [x] Preset picker and save (layout + theme)
- [x] Metric, widget, theme, and sync controls

## Configuration

- [x] `~/.config/fitnessoverlay/settings.json` — app settings (sync offset, max HR, theme, active preset)
- [x] `~/.config/fitnessoverlay/presets/*.json` — reusable layout + theme presets (colours, UI scale, metrics, widgets)

## CLI parity (`./fitnessoverlay --fit … VID_*.mp4`)

- [x] Same overlay pipeline as the editor
- [x] Reads theme from active preset in settings when exporting via CLI

## Bundled ffmpeg

- [x] Build copies `vendor/ffmpeg/<TARGET>/ffmpeg` and `ffprobe` into the build output when present
- [x] Runtime resolves bundled binaries next to the executable, then falls back to PATH
- [ ] Ship pre-built ffmpeg binaries in-repo (place files per `vendor/README.md`)

## Follow-ups

- [ ] One-click export from the GUI
- [ ] Multi-video queue in the editor
- [ ] Windows / Linux CI bundles with vendored ffmpeg
