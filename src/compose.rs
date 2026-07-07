//! ffmpeg composition: pipe raw RGBA overlay frames into ffmpeg, composite
//! them over the source video, and encode the final output.

use crate::video::{rotation_filter, VideoInfo};
use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EncoderPref {
    /// Prefer hardware HEVC (videotoolbox) for short clips, fall back to libx264.
    Auto,
    Hevc,
    H264,
}

fn ffmpeg_has_encoder(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(name))
        .unwrap_or(false)
}

/// Encoder order for `Auto` — long videos skip HEVC (videotoolbox often fails).
pub fn auto_encoder_prefs(duration_secs: f64) -> Vec<EncoderPref> {
    if duration_secs > 600.0 {
        return vec![EncoderPref::H264];
    }
    if ffmpeg_has_encoder("hevc_videotoolbox") {
        vec![EncoderPref::Hevc, EncoderPref::H264]
    } else {
        vec![EncoderPref::H264]
    }
}

/// Resolve encoder preference into concrete ffmpeg args.
pub fn encoder_args(pref: EncoderPref, crf: u8) -> Vec<String> {
    let hevc = vec![
        "-c:v".to_string(),
        "hevc_videotoolbox".to_string(),
        "-q:v".to_string(),
        "55".to_string(),
        "-tag:v".to_string(),
        "hvc1".to_string(),
    ];
    let x264 = vec![
        "-c:v".to_string(),
        "libx264".to_string(),
        "-crf".to_string(),
        crf.to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
    ];
    match pref {
        EncoderPref::Hevc => hevc,
        EncoderPref::H264 => x264,
        EncoderPref::Auto => {
            if ffmpeg_has_encoder("hevc_videotoolbox") {
                hevc
            } else {
                x264
            }
        }
    }
}

/// Options controlling how overlay frames are generated and muxed.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComposeOptions {
    /// When set, generate and pipe overlay frames at this rate instead of every
    /// source frame. Output video is also encoded at this rate (fast preview).
    pub preview_fps: Option<f64>,
}

fn validate_output(path: &Path, expected_duration: f64) -> Result<()> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .context("running ffprobe on encoded output")?;
    if !out.status.success() {
        bail!(
            "encoded output is not a valid video ({}): {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe JSON")?;
    let dur = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    if dur < expected_duration * 0.95 {
        bail!(
            "encoded output {} is too short ({dur:.1}s vs expected {expected_duration:.1}s)",
            path.display()
        );
    }
    Ok(())
}

/// Composite overlay frames produced by `frame_fn` onto `video`, writing the
/// result to `out_path`. `frame_fn(t_video, buf)` fills `buf` with straight
/// RGBA pixels at source time `t_video` in seconds.
pub fn compose(
    video: &VideoInfo,
    out_path: &Path,
    enc_args: &[String],
    opts: ComposeOptions,
    mut frame_fn: impl FnMut(f64, &mut [u8]) -> Result<()>,
) -> Result<()> {
    let overlay_fps = opts.preview_fps.unwrap_or(video.fps);
    if overlay_fps <= 0.0 {
        bail!("overlay frame rate must be positive");
    }
    let fps_str = if opts.preview_fps.is_some() {
        format!("{overlay_fps:.6}")
    } else {
        video.fps_str.clone()
    };
    let total_frames = (video.duration * overlay_fps).ceil().max(1.0) as u64;

    let (dw, dh) = video.display_size();
    let size = format!("{dw}x{dh}");
    let rotate = rotation_filter(video.rotation);
    let filter_complex = if rotate.is_empty() {
        "[0:v][1:v]overlay=eof_action=pass:format=auto[v]".to_string()
    } else {
        format!("[0:v]{rotate}[base];[base][1:v]overlay=eof_action=pass:format=auto[v]")
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "warning", "-nostats", "-y"])
        .arg("-noautorotate")
        .arg("-i")
        .arg(&video.path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-video_size", &size])
        .args(["-framerate", &fps_str])
        .args(["-i", "pipe:0"])
        .args(["-filter_complex", &filter_complex])
        .args(["-map", "[v]"]);
    if video.has_audio {
        cmd.args(["-map", "0:a", "-c:a", "copy"]);
    }
    cmd.args(enc_args).args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
    if opts.preview_fps.is_some() {
        cmd.args(["-r", &fps_str]);
    }
    cmd.arg(out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning ffmpeg (is it installed?)")?;
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stderr = child.stderr.take().expect("child stderr");

    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    let frame_bytes = (dw * dh * 4) as usize;
    let mut buf = vec![0u8; frame_bytes];
    let mut wrote = 0u64;
    let mut broken_pipe = false;

    for idx in 0..total_frames {
        let t_v = idx as f64 / overlay_fps;
        frame_fn(t_v, &mut buf)?;
        match stdin.write_all(&buf) {
            Ok(()) => wrote += 1,
            Err(e) if e.kind() == ErrorKind::BrokenPipe => {
                broken_pipe = true;
                break;
            }
            Err(e) => return Err(e).context("writing overlay frames to ffmpeg"),
        }
        if idx % 30 == 0 || idx + 1 == total_frames {
            eprint!(
                "\r  {} {:>5.1}%",
                out_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                (idx + 1) as f64 / total_frames as f64 * 100.0
            );
        }
    }
    eprintln!();
    drop(stdin);

    let status = child.wait().context("waiting for ffmpeg")?;
    let ffmpeg_stderr = stderr_handle.join().unwrap_or_default();

    if !ffmpeg_stderr.is_empty() {
        for line in ffmpeg_stderr.lines() {
            if line.contains("Error encoding") || line.contains("error") {
                eprintln!("ffmpeg: {line}");
            }
        }
    }

    if !status.success() {
        bail!(
            "ffmpeg exited with {status} while writing {}: {}",
            out_path.display(),
            ffmpeg_stderr.trim()
        );
    }
    if ffmpeg_stderr.contains("Error encoding frame") {
        bail!(
            "ffmpeg encoder failed while writing {}: {}",
            out_path.display(),
            ffmpeg_stderr.trim()
        );
    }
    if broken_pipe && wrote < total_frames * 9 / 10 {
        bail!(
            "ffmpeg stopped accepting frames early ({wrote}/{total_frames}) for {}",
            out_path.display()
        );
    }

    validate_output(out_path, video.duration)?;
    Ok(())
}
