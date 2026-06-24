//! CLI entry: burn overlays onto video files.

use crate::compose;
use crate::config::AppSettings;
use crate::fit;
use crate::layout;
use crate::render;
use crate::video;
use anyhow::{bail, Context, Result};
use chrono::Offset;
use clap::Parser;
use std::path::PathBuf;

/// Burn FIT-activity HUD overlays (noodle map, HR, pace/speed, ...) onto
/// Insta360 vertical videos.
#[derive(Parser, Debug)]
#[command(name = "fitnessoverlay", version, about)]
pub struct Cli {
    /// The .fit activity file recorded by your watch / bike computer.
    #[arg(long)]
    pub fit: PathBuf,

    /// Insta360 video files (VID_YYYYMMDD_HHMMSS_XX_NNN.mp4). The recording
    /// start time is parsed from each filename.
    #[arg(required = true)]
    pub videos: Vec<PathBuf>,

    /// Output directory (created if missing).
    #[arg(long, default_value = "out")]
    pub out: PathBuf,

    /// Manual fine-tune of overlay timing in seconds. Positive shifts the
    /// activity data later relative to the video.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub sync_offset: f64,

    /// UTC offset of the camera's clock, e.g. "+02:00". Defaults to the
    /// offset stored in the FIT file, then the local system timezone.
    #[arg(long)]
    pub utc_offset: Option<String>,

    /// Max heart rate, used for the indoor HR zone bar.
    #[arg(long, default_value_t = 190.0)]
    pub max_hr: f64,

    /// Video encoder selection.
    #[arg(long, value_enum, default_value_t = compose::EncoderPref::Auto)]
    pub encoder: compose::EncoderPref,

    /// Fast preview: render overlay and encode output at this fps (e.g. `1`).
    #[arg(long)]
    pub preview_fps: Option<f64>,

    /// Comma-separated metrics for the bottom HUD row (e.g. pace,hr,distance).
    #[arg(long, value_delimiter = ',')]
    pub metrics: Option<Vec<String>>,

    /// Comma-separated widget set replacing sport defaults (e.g. time,map,metrics).
    #[arg(long, value_delimiter = ',')]
    pub widgets: Option<Vec<String>>,

    /// Enable a widget in addition to sport defaults. Repeatable.
    #[arg(long = "widget")]
    pub enable_widget: Vec<String>,

    /// Disable a widget from sport defaults. Repeatable.
    #[arg(long = "no-widget")]
    pub disable_widget: Vec<String>,
}

/// Parse "+02:00", "-05:30", "+0200" or "2" into seconds.
pub fn parse_utc_offset(s: &str) -> Result<i64> {
    let s = s.trim();
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1i64, &s[1..]),
        Some(b'-') => (-1i64, &s[1..]),
        _ => (1i64, s),
    };
    let (h, m): (i64, i64) = if let Some((h, m)) = rest.split_once(':') {
        (h.parse()?, m.parse()?)
    } else if rest.len() == 4 {
        (rest[0..2].parse()?, rest[2..4].parse()?)
    } else {
        (rest.parse()?, 0)
    };
    if h > 18 || m >= 60 {
        bail!("invalid UTC offset '{s}'");
    }
    Ok(sign * (h * 3600 + m * 60))
}

const FADE_SECS: f64 = 0.5;

/// Overlay opacity ramp. Fades only apply at boundaries caused by the
/// activity starting/ending mid-video, not at the video's own edges.
pub fn fade_at(t: f64, lo: f64, hi: f64, video_duration: f64) -> f32 {
    let f_in = if lo > 0.0 {
        ((t - lo) / FADE_SECS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let f_out = if hi < video_duration {
        ((hi - t) / FADE_SECS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    (f_in.min(f_out)) as f32
}

pub fn run_cli(cli: Cli) -> Result<()> {
    let settings = AppSettings::load().unwrap_or_default();
    let theme = settings.active_theme();

    let timeline = fit::decode(&cli.fit)?;
    let dur = timeline.duration();
    eprintln!(
        "activity: {} | start {} UTC | {} | {} samples | {} pause(s){}",
        timeline.sport.label(),
        timeline.start_utc.format("%Y-%m-%d %H:%M:%S"),
        render::fmt_duration(dur),
        timeline.samples.len(),
        timeline.pauses.len(),
        if timeline.has_gps { " | GPS" } else { "" },
    );

    let utc_offset_secs = match &cli.utc_offset {
        Some(s) => parse_utc_offset(s).context("parsing --utc-offset")?,
        None => timeline.utc_offset_secs.unwrap_or_else(|| {
            let off = timeline
                .start_utc
                .with_timezone(&chrono::Local)
                .offset()
                .fix()
                .local_minus_utc() as i64;
            eprintln!("note: FIT file has no timezone info, assuming system timezone");
            off
        }),
    };
    eprintln!(
        "camera clock UTC offset: {}{:02}:{:02}",
        if utc_offset_secs < 0 { "-" } else { "+" },
        utc_offset_secs.abs() / 3600,
        (utc_offset_secs.abs() % 3600) / 60
    );

    std::fs::create_dir_all(&cli.out)
        .with_context(|| format!("creating output dir {}", cli.out.display()))?;
    let enc_args = compose::encoder_args(cli.encoder);

    let mut rendered = 0usize;
    for path in &cli.videos {
        let info = video::probe(path)?;
        let sync = video::compute_sync(
            info.start_local,
            info.duration,
            utc_offset_secs,
            timeline.start_utc,
            dur,
            cli.sync_offset,
        );
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("video");

        let Some((lo, hi)) = sync.visible else {
            eprintln!(
                "skipping {stem}: no overlap with the activity (video starts {} local)",
                info.start_local
            );
            continue;
        };
        eprintln!(
            "{stem}: {}x{} @ {:.2} fps, {} long; overlay visible {} - {} (activity {} - {}){}",
            info.width,
            info.height,
            info.fps,
            render::fmt_duration(info.duration),
            render::fmt_duration(lo),
            render::fmt_duration(hi),
            render::fmt_duration(lo + sync.offset),
            render::fmt_duration(hi + sync.offset),
            cli.preview_fps
                .map(|p| format!("; preview @ {p:.3} fps"))
                .unwrap_or_default(),
        );

        let mut warnings = Vec::new();
        let overrides = layout::LayoutOverrides::from_parts(
            cli.metrics.clone(),
            cli.widgets.clone(),
            cli.enable_widget.clone(),
            cli.disable_widget.clone(),
            &mut warnings,
        );
        let layout = layout::LayoutConfig::resolve(&timeline, &overrides, cli.max_hr);
        warnings.extend(layout.warnings.clone());
        for w in &warnings {
            eprintln!("warning: {w}");
        }

        let mut renderer = render::OverlayRenderer::new(
            &timeline,
            info.width,
            info.height,
            cli.max_hr,
            &layout,
            &theme,
        )?;
        let out_path = cli.out.join(format!("{stem}_overlay.mp4"));
        let offset = sync.offset;
        let compose_opts = compose::ComposeOptions {
            preview_fps: cli.preview_fps,
        };
        compose::compose(&info, &out_path, &enc_args, compose_opts, |t_v, buf| {
            if t_v < lo || t_v > hi {
                buf.fill(0);
                return Ok(());
            }
            let t_act = t_v + offset;
            let snap = timeline.snapshot(t_act);
            renderer.render_frame(&snap, t_act, fade_at(t_v, lo, hi, info.duration), buf);
            Ok(())
        })?;
        eprintln!("  wrote {}", out_path.display());
        rendered += 1;
    }

    if rendered == 0 {
        bail!("no videos overlapped the activity; check --utc-offset / --sync-offset");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utc_offsets() {
        assert_eq!(parse_utc_offset("+02:00").unwrap(), 7200);
        assert_eq!(parse_utc_offset("-05:30").unwrap(), -19800);
        assert_eq!(parse_utc_offset("+0200").unwrap(), 7200);
        assert_eq!(parse_utc_offset("2").unwrap(), 7200);
        assert_eq!(parse_utc_offset("-7").unwrap(), -25200);
        assert!(parse_utc_offset("+25:00").is_err());
    }

    #[test]
    fn fade_ramps_at_activity_boundaries() {
        let (lo, hi, dur) = (10.0, 20.0, 30.0);
        assert_eq!(fade_at(10.0, lo, hi, dur), 0.0);
        assert!((fade_at(10.25, lo, hi, dur) - 0.5).abs() < 1e-6);
        assert_eq!(fade_at(15.0, lo, hi, dur), 1.0);
        assert!((fade_at(19.75, lo, hi, dur) - 0.5).abs() < 1e-6);
        assert_eq!(fade_at(20.0, lo, hi, dur), 0.0);
    }

    #[test]
    fn no_fade_at_video_edges() {
        assert_eq!(fade_at(0.0, 0.0, 30.0, 30.0), 1.0);
        assert_eq!(fade_at(29.9, 0.0, 30.0, 30.0), 1.0);
    }
}
