//! Insta360 video probing and overlay/activity time synchronization.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub path: PathBuf,
    /// Wall-clock recording start parsed from the filename (device-local time).
    pub start_local: NaiveDateTime,
    pub width: u32,
    pub height: u32,
    /// Exact frame rate as reported by ffprobe (e.g. "30000/1001").
    pub fps_str: String,
    pub fps: f64,
    pub duration: f64,
    pub has_audio: bool,
}

/// Mapping between video time and activity time for one clip:
/// `t_activity = t_video + offset`.
#[derive(Debug, Clone, Copy)]
pub struct SyncMap {
    pub offset: f64,
    /// Video-time interval where activity data exists (overlay visible).
    pub visible: Option<(f64, f64)>,
}

/// Parse `VID_YYYYMMDD_HHMMSS_XX_NNN.mp4` (Insta360 naming) into the local
/// recording start time.
pub fn parse_insta360_start(filename: &str) -> Option<NaiveDateTime> {
    let stem = filename.rsplit('/').next()?;
    let mut parts = stem.split('_');
    let prefix = parts.next()?;
    if !prefix.eq_ignore_ascii_case("VID") && !prefix.eq_ignore_ascii_case("LRV") {
        return None;
    }
    let date = parts.next()?;
    let time = parts.next()?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    let y: i32 = date[0..4].parse().ok()?;
    let mo: u32 = date[4..6].parse().ok()?;
    let d: u32 = date[6..8].parse().ok()?;
    let h: u32 = time[0..2].parse().ok()?;
    let mi: u32 = time[2..4].parse().ok()?;
    let s: u32 = time[4..6].parse().ok()?;
    NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, s)
}

#[derive(Deserialize)]
struct ProbeOut {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: String,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

fn parse_rate(rate: &str) -> Option<f64> {
    let mut it = rate.split('/');
    let num: f64 = it.next()?.trim().parse().ok()?;
    match it.next() {
        Some(den) => {
            let den: f64 = den.trim().parse().ok()?;
            if den == 0.0 {
                None
            } else {
                Some(num / den)
            }
        }
        None => Some(num),
    }
}

pub fn probe(path: &Path) -> Result<VideoInfo> {
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let start_local = parse_insta360_start(filename).with_context(|| {
        format!("cannot parse recording start time from filename '{filename}' (expected Insta360 format VID_YYYYMMDD_HHMMSS_XX_NNN.mp4)")
    })?;

    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .context("running ffprobe (is ffmpeg installed?)")?;
    if !out.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let probe: ProbeOut = serde_json::from_slice(&out.stdout).context("parsing ffprobe json")?;

    let vstream = probe
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .context("no video stream found")?;
    let has_audio = probe.streams.iter().any(|s| s.codec_type == "audio");

    let fps_str = vstream
        .avg_frame_rate
        .clone()
        .filter(|r| parse_rate(r).map(|f| f > 0.0).unwrap_or(false))
        .or_else(|| vstream.r_frame_rate.clone())
        .context("no frame rate reported")?;
    let fps = parse_rate(&fps_str).context("unparseable frame rate")?;
    if fps <= 0.0 {
        bail!("invalid frame rate {fps_str}");
    }

    let duration: f64 = probe
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse().ok())
        .context("no duration reported")?;

    Ok(VideoInfo {
        path: path.to_path_buf(),
        start_local,
        width: vstream.width.context("no width")?,
        height: vstream.height.context("no height")?,
        fps_str,
        fps,
        duration,
        has_audio,
    })
}

/// Compute the video->activity time mapping.
///
/// `t_activity = (video_start_local - utc_offset) - activity_start_utc + t_video + sync_offset`
pub fn compute_sync(
    video_start_local: NaiveDateTime,
    video_duration: f64,
    utc_offset_secs: i64,
    activity_start_utc: DateTime<Utc>,
    activity_duration: f64,
    sync_offset: f64,
) -> SyncMap {
    let video_start_utc = video_start_local - Duration::seconds(utc_offset_secs);
    let base = (video_start_utc - activity_start_utc.naive_utc()).num_milliseconds() as f64
        / 1000.0;
    let offset = base + sync_offset;

    // Activity exists for t_activity in [0, activity_duration]
    // => t_video in [-offset, activity_duration - offset], clipped to the video.
    let lo = (-offset).max(0.0);
    let hi = (activity_duration - offset).min(video_duration);
    let visible = if hi > lo { Some((lo, hi)) } else { None };

    SyncMap { offset, visible }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_insta360_filename() {
        let dt = parse_insta360_start("VID_20260607_170953_00_017.mp4").unwrap();
        assert_eq!(
            dt,
            NaiveDate::from_ymd_opt(2026, 6, 7)
                .unwrap()
                .and_hms_opt(17, 9, 53)
                .unwrap()
        );
        assert!(parse_insta360_start("IMG_1234.mp4").is_none());
        assert!(parse_insta360_start("VID_2026_17.mp4").is_none());
    }

    #[test]
    fn sync_video_starts_before_activity() {
        // Activity starts 17:10:53 local (15:10:53 UTC, +02:00), video at 17:09:53.
        let act_start = Utc.with_ymd_and_hms(2026, 6, 7, 15, 10, 53).unwrap();
        let vid_start = NaiveDate::from_ymd_opt(2026, 6, 7)
            .unwrap()
            .and_hms_opt(17, 9, 53)
            .unwrap();
        let sync = compute_sync(vid_start, 600.0, 7200, act_start, 3600.0, 0.0);
        assert!((sync.offset - -60.0).abs() < 1e-9);
        // Overlay appears 60s into the video, runs to its end.
        let (lo, hi) = sync.visible.unwrap();
        assert!((lo - 60.0).abs() < 1e-9);
        assert!((hi - 600.0).abs() < 1e-9);
    }

    #[test]
    fn sync_video_starts_mid_activity_and_outlives_it() {
        let act_start = Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap();
        // Video starts 30 min into a 40-min activity, records for 20 min.
        let vid_start = NaiveDate::from_ymd_opt(2026, 6, 7)
            .unwrap()
            .and_hms_opt(17, 30, 0)
            .unwrap();
        let sync = compute_sync(vid_start, 1200.0, 7200, act_start, 2400.0, 0.0);
        assert!((sync.offset - 1800.0).abs() < 1e-9);
        let (lo, hi) = sync.visible.unwrap();
        assert!((lo - 0.0).abs() < 1e-9);
        assert!((hi - 600.0).abs() < 1e-9); // overlay cuts when activity ends
    }

    #[test]
    fn sync_no_overlap() {
        let act_start = Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap();
        let vid_start = NaiveDate::from_ymd_opt(2026, 6, 7)
            .unwrap()
            .and_hms_opt(20, 0, 0)
            .unwrap();
        let sync = compute_sync(vid_start, 600.0, 7200, act_start, 3600.0, 0.0);
        assert!(sync.visible.is_none());
    }

    #[test]
    fn sync_manual_offset_shifts_mapping() {
        let act_start = Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap();
        let vid_start = NaiveDate::from_ymd_opt(2026, 6, 7)
            .unwrap()
            .and_hms_opt(17, 0, 0)
            .unwrap();
        let a = compute_sync(vid_start, 600.0, 7200, act_start, 3600.0, 0.0);
        let b = compute_sync(vid_start, 600.0, 7200, act_start, 3600.0, 12.5);
        assert!((b.offset - a.offset - 12.5).abs() < 1e-9);
    }

    #[test]
    fn parses_rates() {
        assert!((parse_rate("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert!((parse_rate("30").unwrap() - 30.0).abs() < 1e-9);
        assert!(parse_rate("0/0").is_none());
    }
}
