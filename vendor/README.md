# Bundled ffmpeg (optional)

Place platform-specific binaries here to embed them in the build output:

```
vendor/ffmpeg/<TARGET>/ffmpeg
vendor/ffmpeg/<TARGET>/ffprobe
```

Example for Apple Silicon macOS:

```
vendor/ffmpeg/aarch64-apple-darwin/ffmpeg
vendor/ffmpeg/aarch64-apple-darwin/ffprobe
```

Download static builds from https://evermeet.cx/ffmpeg/ or build from source.
When binaries are absent, `fitnessoverlay` falls back to `ffmpeg` / `ffprobe` on PATH.

At install time, copy the binaries from `target/release/` next to the `fitnessoverlay`
executable (or into `libexec/`) for distribution bundles.
