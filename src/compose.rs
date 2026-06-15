//! ffmpeg composition: pipe raw RGBA overlay frames into ffmpeg, composite
//! them over the source video, and encode the final output.

use crate::video::VideoInfo;
use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EncoderPref {
    /// Prefer hardware HEVC (videotoolbox), fall back to libx264.
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

/// Resolve encoder preference into concrete ffmpeg args.
pub fn encoder_args(pref: EncoderPref) -> Vec<&'static str> {
    let hevc = vec![
        "-c:v",
        "hevc_videotoolbox",
        "-q:v",
        "55",
        "-tag:v",
        "hvc1",
    ];
    let x264 = vec!["-c:v", "libx264", "-crf", "18", "-preset", "veryfast"];
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
#[derive(Debug, Clone, Copy)]
pub struct ComposeOptions {
    /// When set, generate and pipe overlay frames at this rate instead of every
    /// source frame. Output video is also encoded at this rate (fast preview).
    pub preview_fps: Option<f64>,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        Self { preview_fps: None }
    }
}

/// Composite overlay frames produced by `frame_fn` onto `video`, writing the
/// result to `out_path`. `frame_fn(t_video, buf)` fills `buf` with straight
/// RGBA pixels at source time `t_video` in seconds.
pub fn compose(
    video: &VideoInfo,
    out_path: &Path,
    enc_args: &[&str],
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

    let size = format!("{}x{}", video.width, video.height);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostats", "-y"])
        .arg("-i")
        .arg(&video.path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-video_size", &size])
        .args(["-framerate", &fps_str])
        .args(["-i", "pipe:0"])
        .args([
            "-filter_complex",
            "[0:v][1:v]overlay=eof_action=pass:format=auto[v]",
        ])
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
        .stdout(Stdio::null());

    let mut child = cmd.spawn().context("spawning ffmpeg (is it installed?)")?;
    let mut stdin = child.stdin.take().expect("child stdin");

    let frame_bytes = (video.width * video.height * 4) as usize;
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
    if !status.success() {
        bail!("ffmpeg exited with {status} while writing {}", out_path.display());
    }
    if broken_pipe && wrote < total_frames * 9 / 10 {
        bail!(
            "ffmpeg stopped accepting frames early ({wrote}/{total_frames}) for {}",
            out_path.display()
        );
    }
    Ok(())
}
