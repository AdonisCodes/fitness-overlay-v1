//! Video frame extraction and overlay compositing for the GUI preview.

use crate::bundled;
use crate::render;
use anyhow::{Context, Result};
use std::path::Path;

/// Extract one RGB frame from a video at `t` seconds, scaled to fit `max_dim`.
pub fn extract_video_frame(path: &Path, t: f64, max_dim: u32) -> Result<(Vec<u8>, u32, u32)> {
    let (sw, sh) = scaled_dimensions(path, max_dim)?;
    let mut cmd = bundled::ffmpeg_command();
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &format!("{t:.3}"),
        "-i",
    ]);
    cmd.arg(path);
    cmd.args([
        "-vframes",
        "1",
        "-vf",
        &format!("scale={sw}:{sh}:flags=lanczos"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]);
    let out = cmd
        .output()
        .with_context(|| format!("ffmpeg frame extract for {}", path.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "ffmpeg frame extract failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let expected = (sw * sh * 3) as usize;
    if out.stdout.len() < expected {
        anyhow::bail!(
            "short ffmpeg frame read: got {} want {}",
            out.stdout.len(),
            expected
        );
    }
    Ok((out.stdout[..expected].to_vec(), sw, sh))
}

fn scaled_dimensions(path: &Path, max_dim: u32) -> Result<(u32, u32)> {
    let info = crate::video::probe(path)?;
    let w = info.width;
    let h = info.height;
    if w <= max_dim && h <= max_dim {
        return Ok((w, h));
    }
    let scale = max_dim as f32 / w.max(h) as f32;
    let sw = ((w as f32 * scale).round() as u32).max(2) & !1;
    let sh = ((h as f32 * scale).round() as u32).max(2) & !1;
    Ok((sw, sh))
}

/// Alpha-composite straight RGBA overlay over RGB24 video pixels.
pub fn composite_rgba_over_rgb(video_rgb: &mut [u8], overlay_rgba: &[u8]) {
    let n = video_rgb.len() / 3;
    for i in 0..n {
        let a = overlay_rgba[i * 4 + 3] as f32 / 255.0;
        if a <= 0.0 {
            continue;
        }
        let inv = 1.0 - a;
        for c in 0..3 {
            let vi = i * 3 + c;
            let oi = i * 4 + c;
            video_rgb[vi] =
                (overlay_rgba[oi] as f32 * a + video_rgb[vi] as f32 * inv).round() as u8;
        }
    }
}

/// Render overlay RGBA for a frame and composite onto `video_rgb` (same dimensions).
pub fn render_preview_composite(
    renderer: &mut render::OverlayRenderer,
    snap: &crate::fit::Snapshot,
    t_act: f64,
    fade: f32,
    video_rgb: &mut [u8],
    width: u32,
    height: u32,
) {
    let mut overlay = vec![0u8; (width * height * 4) as usize];
    renderer.render_frame(snap, t_act, fade, &mut overlay);
    if video_rgb.len() / 3 == overlay.len() / 4 {
        composite_rgba_over_rgb(video_rgb, &overlay);
    }
}
