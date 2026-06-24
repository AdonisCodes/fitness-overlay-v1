//! The overlay renderer.
//!
//! Efficiency model: everything that never changes (panels, labels, icons,
//! the full route noodle, the elevation profile, the HR zone bar) is
//! rasterized once into a static full-frame layer. Each video frame is then
//! a memcpy of that layer plus a handful of cheap dynamic draws: cached-glyph
//! text blits, the newly traveled noodle segment (incremental, drawn into a
//! persistent pixmap), and 2-3 marker dots.

pub mod map;
pub mod text;

use crate::fit::{Snapshot, SportKind, Timeline};
use crate::layout::{LayoutConfig, MetricId};
use anyhow::{Context, Result};
use map::Track;
use text::GlyphCache;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

const FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-Bold.otf");
const FONT_SEMI: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.otf");

/// Design space is 1080 units on the short side; scale from min(width, height)
/// so landscape and portrait clips get similarly sized HUD elements.
const DESIGN_W: f32 = 1080.0;
const MARGIN: f32 = 48.0;
/// Landscape clips use a compact bottom-left HUD instead of a full-width bar.
const LANDSCAPE_PANEL_WIDTH_FRAC: f32 = 0.38;
/// Minimum gap between the rightmost unit ink and the column divider.
const UNIT_DIVIDER_GAP: f32 = 20.0;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const LABEL: [f32; 4] = [1.0, 1.0, 1.0, 0.72];
const UNIT: [f32; 4] = [1.0, 1.0, 1.0, 0.68];
const ACCENT: (u8, u8, u8) = (255, 122, 26);
const PANEL_BG: (u8, u8, u8, u8) = (10, 12, 18, 115);
const ZONE_COLORS: [(u8, u8, u8); 5] = [
    (132, 138, 148), // Z1 grey
    (66, 133, 244),  // Z2 blue
    (52, 168, 83),   // Z3 green
    (255, 122, 26),  // Z4 orange (accent)
    (217, 48, 37),   // Z5 red
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Pace,
    Speed,
    HeartRate,
    Distance,
    Cadence,
    Power,
    ElevGain,
    Altitude,
}

impl MetricKind {
    fn label(&self) -> &'static str {
        match self {
            MetricKind::Pace => "PACE",
            MetricKind::Speed => "SPEED",
            MetricKind::HeartRate => "HEART RATE",
            MetricKind::Distance => "DISTANCE",
            MetricKind::Cadence => "CADENCE",
            MetricKind::Power => "POWER",
            MetricKind::ElevGain => "ELEV GAIN",
            MetricKind::Altitude => "ALTITUDE",
        }
    }

    fn unit(&self, sport: SportKind) -> &'static str {
        match self {
            MetricKind::Pace => "/km",
            MetricKind::Speed => "km/h",
            MetricKind::HeartRate => "bpm",
            MetricKind::Distance => "km",
            MetricKind::Cadence => {
                if sport == SportKind::BikeRide {
                    "rpm"
                } else {
                    "spm"
                }
            }
            MetricKind::Power => "W",
            MetricKind::ElevGain | MetricKind::Altitude => "m",
        }
    }

    fn format(&self, snap: &Snapshot, sport: SportKind) -> String {
        match self {
            MetricKind::Pace => snap
                .speed
                .map(fmt_pace)
                .unwrap_or_else(|| "--:--".to_string()),
            MetricKind::Speed => snap
                .speed
                .map(|v| format!("{:.1}", v * 3.6))
                .unwrap_or_else(|| "--".to_string()),
            MetricKind::HeartRate => snap
                .hr
                .map(|v| format!("{}", v.round() as i64))
                .unwrap_or_else(|| "--".to_string()),
            MetricKind::Distance => snap
                .dist
                .map(fmt_distance_km)
                .unwrap_or_else(|| "--".to_string()),
            MetricKind::Cadence => snap
                .cadence
                .map(|c| {
                    // FIT run/walk cadence is per-leg; display steps per minute.
                    let v = if sport == SportKind::BikeRide { c } else { c * 2.0 };
                    format!("{}", v.round() as i64)
                })
                .unwrap_or_else(|| "--".to_string()),
            MetricKind::Power => snap
                .power
                .map(|v| format!("{}", v.round() as i64))
                .unwrap_or_else(|| "--".to_string()),
            MetricKind::ElevGain => format!("+{}", fmt_thousands(snap.ascent.round() as i64)),
            MetricKind::Altitude => snap
                .alt
                .map(|v| fmt_thousands(v.round() as i64))
                .unwrap_or_else(|| "--".to_string()),
        }
    }
}

impl MetricKind {
    pub fn from_id(id: MetricId) -> Self {
        match id {
            MetricId::Pace => MetricKind::Pace,
            MetricId::Speed => MetricKind::Speed,
            MetricId::HeartRate => MetricKind::HeartRate,
            MetricId::Distance => MetricKind::Distance,
            MetricId::Cadence => MetricKind::Cadence,
            MetricId::Power => MetricKind::Power,
            MetricId::ElevGain => MetricKind::ElevGain,
            MetricId::Altitude => MetricKind::Altitude,
        }
    }
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

pub fn fmt_pace(speed_ms: f64) -> String {
    if speed_ms < 0.4 {
        return "--:--".to_string();
    }
    let spk = 1000.0 / speed_ms;
    if spk >= 30.0 * 60.0 {
        return "--:--".to_string();
    }
    let m = (spk / 60.0).floor() as u64;
    let s = (spk - m as f64 * 60.0).round() as u64;
    let (m, s) = if s == 60 { (m + 1, 0) } else { (m, s) };
    format!("{m}:{s:02}")
}

pub fn fmt_distance_km(meters: f64) -> String {
    let km = meters / 1000.0;
    if km >= 100.0 {
        format!("{km:.0}")
    } else if km >= 10.0 {
        format!("{km:.1}")
    } else {
        format!("{km:.2}")
    }
}

pub fn fmt_thousands(v: i64) -> String {
    let neg = v < 0;
    let digits = v.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

struct Cell {
    kind: MetricKind,
    /// Left edge of this column in frame pixels.
    x: f32,
    /// Column width in frame pixels.
    w: f32,
    /// Horizontal inset from both column edges for labels/values.
    pad: f32,
    /// Extra inset for the unit's right edge (accounts for glyph ink + divider gap).
    unit_inset: f32,
    value_baseline: f32,
    value_px: f32,
    unit_px: f32,
}

struct TimeChip {
    text_x: f32,
    baseline: f32,
    px: f32,
    // Geometry for the dynamically drawn PAUSED chip.
    chip_y: f32,
    chip_h: f32,
    paused_x: f32,
}

struct MapWidget {
    x: f32,
    y: f32,
    track: Track,
    traveled: Pixmap,
    committed: usize,
    last_pt: Option<(f32, f32)>,
    stroke_w: f32,
    dot_r: f32,
}

struct ElevWidget {
    origin_x: f32,
    origin_y: f32,
    track: Track,
    dot_r: f32,
}

struct ZoneWidget {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    marker_r: f32,
}

pub struct OverlayRenderer {
    sport: SportKind,
    max_hr: f64,
    static_layer: Pixmap,
    frame: Pixmap,
    bold: GlyphCache,
    semi: GlyphCache,
    time_chip: TimeChip,
    cells: Vec<Cell>,
    map: Option<MapWidget>,
    elev: Option<ElevWidget>,
    zone: Option<ZoneWidget>,
}

impl OverlayRenderer {
    pub fn new(
        tl: &Timeline,
        width: u32,
        height: u32,
        max_hr: f64,
        layout: &LayoutConfig,
    ) -> Result<OverlayRenderer> {
        let w = width as f32;
        let h = height as f32;
        let landscape = w > h;
        // Scale from the short side so a 1920×1080 clip doesn't blow up vs 1080×1920.
        let s = w.min(h) / DESIGN_W;
        let mut static_layer =
            Pixmap::new(width, height).context("allocating static layer pixmap")?;
        let frame = Pixmap::new(width, height).context("allocating frame pixmap")?;
        let mut bold = GlyphCache::new(FONT_BOLD);
        let mut semi = GlyphCache::new(FONT_SEMI);

        // ---- time chip (top-left) ----
        let chip_px = 54.0 * s;
        let chip_h = 96.0 * s;
        let chip_x = MARGIN * s;
        let chip_y = MARGIN * s;
        let template = if tl.duration() >= 3600.0 { "8:88:88" } else { "88:88" };
        let text_w = bold.measure(template, chip_px, true, 0.0);
        let pad_x = 30.0 * s;
        let chip_w = text_w + 2.0 * pad_x;
        fill_rrect(
            &mut static_layer,
            chip_x,
            chip_y,
            chip_w,
            chip_h,
            22.0 * s,
            PANEL_BG,
        );
        let time_chip = TimeChip {
            text_x: chip_x + pad_x,
            baseline: chip_y + chip_h / 2.0 + 0.36 * chip_px,
            px: chip_px,
            chip_y,
            chip_h,
            paused_x: chip_x + chip_w + 16.0 * s,
        };

        // ---- bottom metric panel ----
        let kinds: Vec<MetricKind> = layout
            .metrics
            .iter()
            .map(|&m| MetricKind::from_id(m))
            .collect();
        let bottom_gap = if landscape { MARGIN * s } else { 120.0 * s };
        let (panel_x, panel_w, panel_h, value_px, label_px, unit_px, cell_pad) = if landscape {
            let panel_w = w * LANDSCAPE_PANEL_WIDTH_FRAC;
            (
                MARGIN * s,
                panel_w,
                160.0 * s,
                52.0 * s,
                20.0 * s,
                24.0 * s,
                20.0 * s,
            )
        } else {
            (
                MARGIN * s,
                w - 2.0 * MARGIN * s,
                200.0 * s,
                72.0 * s,
                24.0 * s,
                30.0 * s,
                25.0 * s,
            )
        };
        let panel_y = h - bottom_gap - panel_h;
        let mut cells = Vec::new();
        if layout.widgets.metrics_panel && !kinds.is_empty() {
            fill_rrect(
                &mut static_layer,
                panel_x,
                panel_y,
                panel_w,
                panel_h,
                if landscape { 22.0 * s } else { 28.0 * s },
                PANEL_BG,
            );
            let n = kinds.len();
            let cw = panel_w / n as f32;
            for (i, &kind) in kinds.iter().enumerate() {
                let cx = panel_x + i as f32 * cw;
                let mut text_x = cx + cell_pad;
                let label_baseline = panel_y + if landscape { 52.0 * s } else { 64.0 * s };
                // Divider between cells.
                if i > 0 {
                    stroke_line(
                        &mut static_layer,
                        cx,
                        panel_y + if landscape { 30.0 * s } else { 38.0 * s },
                        cx,
                        panel_y + if landscape { 130.0 * s } else { 162.0 * s },
                        2.0 * s,
                        (255, 255, 255, 30),
                    );
                }
                // Label icons.
                let icon_size = if landscape { 20.0 * s } else { 26.0 * s };
                let icon_y = label_baseline - icon_size + 3.0 * s;
                match kind {
                    MetricKind::HeartRate => {
                        draw_heart(&mut static_layer, text_x, icon_y, icon_size, (229, 57, 53, 255));
                        text_x += icon_size + 8.0 * s;
                    }
                    MetricKind::ElevGain => {
                        draw_mountain(
                            &mut static_layer,
                            text_x,
                            icon_y,
                            icon_size,
                            (255, 255, 255, 215),
                        );
                        text_x += icon_size + 8.0 * s;
                    }
                    _ => {}
                }
                semi.draw(
                    &mut static_layer,
                    kind.label(),
                    text_x,
                    label_baseline,
                    label_px,
                    LABEL,
                    false,
                    if landscape { 2.0 * s } else { 3.0 * s },
                );
                cells.push(Cell {
                    kind,
                    x: cx,
                    w: cw,
                    pad: cell_pad,
                    unit_inset: UNIT_DIVIDER_GAP * s,
                    value_baseline: panel_y + if landscape { 124.0 * s } else { 152.0 * s },
                    value_px,
                    unit_px,
                });
            }
        }

        // ---- noodle map (top-right) ----
        let map = if layout.widgets.map {
            let size = if landscape { 280.0 * s } else { 340.0 * s };
            let mx = w - MARGIN * s - size;
            let my = MARGIN * s;
            Track::from_gps(&tl.samples, size, size, 26.0 * s).map(|track| {
                let stroke_w = 5.5 * s;
                let full = track_path(&track, mx, my);
                stroke_path_color(
                    &mut static_layer,
                    &full,
                    stroke_w + 4.0 * s,
                    (0, 0, 0, 80),
                );
                stroke_path_color(&mut static_layer, &full, stroke_w, (255, 255, 255, 235));
                fill_circle(
                    &mut static_layer,
                    mx + track.xs[0],
                    my + track.ys[0],
                    5.0 * s,
                    (255, 255, 255, 255),
                );
                let traveled = Pixmap::new(size.ceil() as u32, size.ceil() as u32)
                    .expect("traveled pixmap");
                let first = (track.xs[0], track.ys[0]);
                MapWidget {
                    x: mx,
                    y: my,
                    track,
                    traveled,
                    committed: 0,
                    last_pt: Some(first),
                    stroke_w,
                    dot_r: 9.0 * s,
                }
            })
        } else {
            None
        };

        let mut next_y = if layout.widgets.metrics_panel {
            panel_y
        } else {
            h - bottom_gap
        } - 16.0 * s;

        // ---- HR zone bar ----
        let zone = if layout.widgets.hr_zones {
            let zh = 24.0 * s;
            let zy = next_y - zh;
            let gap = 6.0 * s;
            let seg_w = (panel_w - 4.0 * gap) / 5.0;
            for (i, &(r, g, b)) in ZONE_COLORS.iter().enumerate() {
                let zx = panel_x + i as f32 * (seg_w + gap);
                fill_rrect(&mut static_layer, zx, zy, seg_w, zh, zh / 2.0, (r, g, b, 217));
            }
            next_y = zy - 12.0 * s; // Move up for the next widget
            Some(ZoneWidget {
                x: panel_x,
                y: zy,
                w: panel_w,
                h: zh,
                marker_r: 11.0 * s,
            })
        } else {
            None
        };

        // ---- elevation profile ----
        let elev = if layout.widgets.elevation {
            let eh = 120.0 * s;
            let ey = next_y - eh;
            let pad = 18.0 * s;
            let inner_w = panel_w - 2.0 * pad;
            let inner_h = eh - 2.0 * pad;
            Track::elevation_profile(&tl.samples, inner_w, inner_h, 0.0).map(|track| {
                fill_rrect(
                    &mut static_layer,
                    panel_x,
                    ey,
                    panel_w,
                    eh,
                    24.0 * s,
                    PANEL_BG,
                );
                let ox = panel_x + pad;
                let oy = ey + pad;
                // Filled area under the profile.
                if let Some(area) = area_path(&track, ox, oy, inner_h) {
                    fill_path_color(&mut static_layer, &area, (255, 255, 255, 60));
                }
                let line = track_path(&track, ox, oy);
                stroke_path_color(&mut static_layer, &line, 3.0 * s, (255, 255, 255, 215));
                ElevWidget {
                    origin_x: ox,
                    origin_y: oy,
                    track,
                    dot_r: 7.0 * s,
                }
            })
        } else {
            None
        };

        Ok(OverlayRenderer {
            sport: tl.sport,
            max_hr,
            static_layer,
            frame,
            bold,
            semi,
            time_chip,
            cells,
            map,
            elev,
            zone,
        })
    }

    /// Render the overlay at activity time `t_act` into `out` as straight
    /// (non-premultiplied) RGBA. `fade` in 0..=1 scales the overall opacity.
    pub fn render_frame(&mut self, snap: &Snapshot, t_act: f64, fade: f32, out: &mut [u8]) {
        self.frame
            .data_mut()
            .copy_from_slice(self.static_layer.data());

        // Elapsed (moving) time + paused indicator.
        let time_text = fmt_duration(snap.moving_secs);
        self.bold.draw(
            &mut self.frame,
            &time_text,
            self.time_chip.text_x,
            self.time_chip.baseline,
            self.time_chip.px,
            WHITE,
            true,
            0.0,
        );
        if snap.paused {
            let px = self.time_chip.px * 0.52;
            let tw = self.semi.measure("PAUSED", px, false, 1.5);
            let pad = self.time_chip.chip_h * 0.28;
            fill_rrect(
                &mut self.frame,
                self.time_chip.paused_x,
                self.time_chip.chip_y,
                tw + 2.0 * pad,
                self.time_chip.chip_h,
                self.time_chip.chip_h * 0.23,
                (ACCENT.0, ACCENT.1, ACCENT.2, 230),
            );
            self.semi.draw(
                &mut self.frame,
                "PAUSED",
                self.time_chip.paused_x + pad,
                self.time_chip.baseline - self.time_chip.px * 0.06,
                px,
                WHITE,
                false,
                1.5,
            );
        }

        // Metric cells — value + unit flow left-to-right; clamp so unit ink
        // never crosses the divider gutter on the right.
        for cell in &self.cells {
            let value = cell.kind.format(snap, self.sport);
            let unit = cell.kind.unit(self.sport);
            let left = cell.x + cell.pad;
            let right_ink_limit = cell.x + cell.w - cell.unit_inset;
            let (_, unit_ink_w) = self.semi.measure_extents(unit, cell.unit_px, false, 0.0);
            let (value_w, _) = self.bold.measure_extents(&value, cell.value_px, true, 0.0);
            let gap = 8.0 * (cell.value_px / 52.0);
            let mut unit_x = left + value_w + gap;
            if unit_x + unit_ink_w > right_ink_limit {
                unit_x = (right_ink_limit - unit_ink_w).max(left + value_w + gap);
            }
            let value_x = (unit_x - gap - value_w).max(left);
            self.bold.draw(
                &mut self.frame,
                &value,
                value_x,
                cell.value_baseline,
                cell.value_px,
                WHITE,
                true,
                0.0,
            );
            self.semi.draw(
                &mut self.frame,
                unit,
                unit_x,
                cell.value_baseline,
                cell.unit_px,
                UNIT,
                false,
                0.0,
            );
        }

        // Noodle map: commit newly traveled segments, blit, draw position dot.
        if let Some(m) = self.map.as_mut() {
            let cur = m.track.point_at(t_act);
            let idx = m.track.index_at(t_act);
            if m.committed != idx || m.last_pt != Some(cur) {
                let mut pb = PathBuilder::new();
                let start = m.last_pt.unwrap_or(cur);
                pb.move_to(start.0, start.1);
                for i in (m.committed + 1)..=idx {
                    pb.line_to(m.track.xs[i], m.track.ys[i]);
                }
                pb.line_to(cur.0, cur.1);
                if let Some(path) = pb.finish() {
                    stroke_path_color(
                        &mut m.traveled,
                        &path,
                        m.stroke_w,
                        (ACCENT.0, ACCENT.1, ACCENT.2, 255),
                    );
                }
                m.committed = idx;
                m.last_pt = Some(cur);
            }
            self.frame.draw_pixmap(
                m.x as i32,
                m.y as i32,
                m.traveled.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            let (cx, cy) = (m.x + cur.0, m.y + cur.1);
            fill_circle(&mut self.frame, cx, cy, m.dot_r, (ACCENT.0, ACCENT.1, ACCENT.2, 255));
            stroke_circle(&mut self.frame, cx, cy, m.dot_r, m.stroke_w * 0.55, (255, 255, 255, 255));
        }

        // Elevation profile marker.
        if let Some(e) = &self.elev {
            let (x, y) = e.track.point_at(t_act);
            let (cx, cy) = (e.origin_x + x, e.origin_y + y);
            fill_circle(&mut self.frame, cx, cy, e.dot_r, (ACCENT.0, ACCENT.1, ACCENT.2, 255));
            stroke_circle(&mut self.frame, cx, cy, e.dot_r, 2.5, (255, 255, 255, 255));
        }

        // HR zone marker.
        if let Some(z) = &self.zone
            && let Some(hr) = snap.hr
        {
            let frac = (((hr / self.max_hr) - 0.5) / 0.5).clamp(0.0, 1.0) as f32;
            let cx = z.x + frac * z.w;
            let cy = z.y + z.h / 2.0;
            fill_circle(&mut self.frame, cx, cy, z.marker_r, (255, 255, 255, 255));
            stroke_circle(&mut self.frame, cx, cy, z.marker_r, 3.0, (0, 0, 0, 70));
        }

        write_rgba_straight(&self.frame, fade, out);
    }
}


// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn color8(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.5523 * r; // cubic circle approximation
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_rrect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: (u8, u8, u8, u8)) {
    if let Some(path) = rounded_rect_path(x, y, w, h, r) {
        fill_path_color(pixmap, &path, c);
    }
}

fn fill_path_color(pixmap: &mut Pixmap, path: &Path, c: (u8, u8, u8, u8)) {
    let mut paint = Paint::default();
    paint.set_color(color8(c.0, c.1, c.2, c.3));
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_path_color(pixmap: &mut Pixmap, path: &Path, width: f32, c: (u8, u8, u8, u8)) {
    let mut paint = Paint::default();
    paint.set_color(color8(c.0, c.1, c.2, c.3));
    paint.anti_alias = true;
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
}

fn stroke_line(pixmap: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: (u8, u8, u8, u8)) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(path) = pb.finish() {
        stroke_path_color(pixmap, &path, w, c);
    }
}

fn fill_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, c: (u8, u8, u8, u8)) {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        fill_path_color(pixmap, &path, c);
    }
}

fn stroke_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, w: f32, c: (u8, u8, u8, u8)) {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        stroke_path_color(pixmap, &path, w, c);
    }
}

/// Track polyline translated by (ox, oy).
fn track_path(track: &Track, ox: f32, oy: f32) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(ox + track.xs[0], oy + track.ys[0]);
    for i in 1..track.xs.len() {
        pb.line_to(ox + track.xs[i], oy + track.ys[i]);
    }
    pb.finish().expect("track path")
}

/// Closed area under a profile polyline (down to `floor_h`).
fn area_path(track: &Track, ox: f32, oy: f32, floor_h: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(ox + track.xs[0], oy + floor_h);
    for i in 0..track.xs.len() {
        pb.line_to(ox + track.xs[i], oy + track.ys[i]);
    }
    pb.line_to(ox + track.xs[track.xs.len() - 1], oy + floor_h);
    pb.close();
    pb.finish()
}

fn draw_heart(pixmap: &mut Pixmap, x: f32, y: f32, size: f32, c: (u8, u8, u8, u8)) {
    let u = |v: f32| v * size;
    let mut pb = PathBuilder::new();
    pb.move_to(x + u(0.5), y + u(0.91));
    pb.cubic_to(x + u(0.18), y + u(0.64), x + u(0.05), y + u(0.46), x + u(0.05), y + u(0.30));
    pb.cubic_to(x + u(0.05), y + u(0.12), x + u(0.18), y + u(0.02), x + u(0.32), y + u(0.02));
    pb.cubic_to(x + u(0.41), y + u(0.02), x + u(0.47), y + u(0.06), x + u(0.5), y + u(0.13));
    pb.cubic_to(x + u(0.53), y + u(0.06), x + u(0.59), y + u(0.02), x + u(0.68), y + u(0.02));
    pb.cubic_to(x + u(0.82), y + u(0.02), x + u(0.95), y + u(0.12), x + u(0.95), y + u(0.30));
    pb.cubic_to(x + u(0.95), y + u(0.46), x + u(0.82), y + u(0.64), x + u(0.5), y + u(0.91));
    pb.close();
    if let Some(path) = pb.finish() {
        fill_path_color(pixmap, &path, c);
    }
}

fn draw_mountain(pixmap: &mut Pixmap, x: f32, y: f32, size: f32, c: (u8, u8, u8, u8)) {
    let u = |v: f32| v * size;
    let mut pb = PathBuilder::new();
    pb.move_to(x, y + u(0.95));
    pb.line_to(x + u(0.36), y + u(0.18));
    pb.line_to(x + u(0.55), y + u(0.58));
    pb.line_to(x + u(0.70), y + u(0.36));
    pb.line_to(x + u(1.0), y + u(0.95));
    pb.close();
    if let Some(path) = pb.finish() {
        fill_path_color(pixmap, &path, c);
    }
}

/// Convert premultiplied pixmap data into straight RGBA, applying a global
/// `fade` to the alpha channel. Transparent and opaque pixels take fast paths.
pub fn write_rgba_straight(pixmap: &Pixmap, fade: f32, out: &mut [u8]) {
    let src = pixmap.data();
    debug_assert_eq!(src.len(), out.len());
    let fade = fade.clamp(0.0, 1.0);
    for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let a = s[3];
        if a == 0 {
            d.copy_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
            d[3] = (255.0 * fade) as u8;
        } else {
            let af = a as f32;
            d[0] = ((s[0] as f32 * 255.0 / af).min(255.0)) as u8;
            d[1] = ((s[1] as f32 * 255.0 / af).min(255.0)) as u8;
            d[2] = ((s[2] as f32 * 255.0 / af).min(255.0)) as u8;
            d[3] = (af * fade) as u8;
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landscape_panel_is_compact_bottom_left() {
        use crate::fit::{Sample, Timeline};
        use chrono::TimeZone;

        let tl = Timeline {
            start_utc: chrono::Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport: SportKind::OutdoorRun,
            utc_offset_secs: None,
            samples: vec![Sample {
                t: 0.0,
                hr: Some(120.0),
                dist: Some(0.0),
                speed: Some(3.0),
                lat: Some(47.0),
                lon: Some(8.0),
                ..Default::default()
            }],
            pauses: vec![],
            has_gps: true,
        };
        let (w, h) = (2752u32, 1530u32);
        let panel_w = w as f32 * LANDSCAPE_PANEL_WIDTH_FRAC;
        assert!(panel_w < w as f32 * 0.5);
        let s = w.min(h) as f32 / DESIGN_W;
        let panel_h = 160.0 * s;
        let panel_y = h as f32 - MARGIN * s - panel_h;
        assert!(panel_y + panel_h <= h as f32);
        assert!(panel_h < 240.0, "panel too tall: {panel_h}");
        let _ = OverlayRenderer::new(
            &tl,
            w,
            h,
            190.0,
            &LayoutConfig::resolve(&tl, &Default::default(), 190.0),
        )
        .unwrap();
    }

    #[test]
    fn metric_units_clear_cell_dividers() {
        use crate::fit::{Sample, Timeline};
        use chrono::TimeZone;

        fn check(tl: &Timeline, w: u32, h: u32) {
            let s = w.min(h) as f32 / DESIGN_W;
            let panel_x = MARGIN * s;
            let panel_w = if w > h {
                w as f32 * LANDSCAPE_PANEL_WIDTH_FRAC
            } else {
                w as f32 - 2.0 * MARGIN * s
            };
            let layout = LayoutConfig::resolve(tl, &Default::default(), 190.0);
            let n = layout.metrics.len().max(1);
            let cw = panel_w / n as f32;
            let min_gap = (UNIT_DIVIDER_GAP * s * 0.85).round() as u32;
            let y0 = if w > h {
                (h as f32 - MARGIN * s - 160.0 * s + 100.0 * s) as u32
            } else {
                (h as f32 - 120.0 * s - 200.0 * s + 128.0 * s) as u32
            };
            let y1 = y0 + 40;

            let mut r = OverlayRenderer::new(&tl, w, h, 190.0, &layout).unwrap();
            let snap = tl.snapshot(tl.duration() * 0.62);
            let mut out = vec![0u8; (w * h * 4) as usize];
            r.render_frame(&snap, tl.duration() * 0.62, 1.0, &mut out);

            for i in 0..n.saturating_sub(1) {
                let div = (panel_x + (i as f32 + 1.0) * cw).round() as u32;
                let x0 = (panel_x + i as f32 * cw).round() as u32;
                let mut max_x = x0;
                for y in y0..y1 {
                    for x in x0..div.saturating_sub(min_gap) {
                        if out[((y * w + x) * 4 + 3) as usize] > 0 {
                            max_x = max_x.max(x);
                        }
                    }
                }
                assert!(
                    max_x <= div - min_gap,
                    "{:?} {}x{} cell {i}: ink to {max_x}, divider {div}, need {min_gap}px",
                    tl.sport,
                    w,
                    h
                );
            }
        }

        let base = |sport: SportKind| Timeline {
            start_utc: chrono::Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport,
            utc_offset_secs: None,
            samples: vec![Sample {
                t: 0.0,
                hr: Some(125.0),
                dist: Some(9510.0),
                speed: Some(9.36),
                power: Some(245.0),
                cadence: Some(176.0),
                lat: Some(47.0),
                lon: Some(8.0),
                ..Default::default()
            }],
            pauses: vec![],
            has_gps: sport != SportKind::IndoorRun,
        };

        for sport in [
            SportKind::OutdoorRun,
            SportKind::IndoorRun,
            SportKind::BikeRide,
            SportKind::Hike,
        ] {
            check(&base(sport), 1080, 1920);
            check(&base(sport), 2752, 1530);
        }
    }

    #[test]
    fn bike_landscape_unit_clears_divider() {
        use crate::fit::{Sample, Timeline};
        use chrono::TimeZone;

        let tl = Timeline {
            start_utc: chrono::Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport: SportKind::BikeRide,
            utc_offset_secs: None,
            samples: vec![Sample {
                t: 0.0,
                hr: Some(125.0),
                dist: Some(9510.0),
                speed: Some(9.36),
                power: Some(245.0),
                lat: Some(47.0),
                lon: Some(8.0),
                ..Default::default()
            }],
            pauses: vec![],
            has_gps: true,
        };
        let (w, h) = (2752u32, 1530u32);
        let s = w.min(h) as f32 / DESIGN_W;
        let panel_w = w as f32 * LANDSCAPE_PANEL_WIDTH_FRAC;
        let panel_x = MARGIN * s;
        let cw = panel_w / 4.0;
        let divider_x = (panel_x + cw).round() as u32;
        let gap_px = (UNIT_DIVIDER_GAP * s * 0.85).round() as u32;

        let mut r = OverlayRenderer::new(
            &tl,
            w,
            h,
            190.0,
            &LayoutConfig::resolve(&tl, &Default::default(), 190.0),
        )
        .unwrap();
        let snap = tl.snapshot(0.0);
        let mut out = vec![0u8; (w * h * 4) as usize];
        r.render_frame(&snap, 0.0, 1.0, &mut out);

        let mut max_unit_x = 0u32;
        // Scan a band around the value baseline in cell 0 (speed).
        let y0 = (h as f32 - MARGIN * s - 160.0 * s + 100.0 * s) as u32;
        let y1 = y0 + 40;
        for y in y0..y1 {
            for x in 0..divider_x.saturating_sub(gap_px) {
                let a = out[((y * w + x) * 4 + 3) as usize];
                if a > 0 {
                    max_unit_x = max_unit_x.max(x);
                }
            }
        }
        assert!(
            max_unit_x <= divider_x - gap_px,
            "unit ink reaches x={max_unit_x}, divider at {divider_x}, wanted {gap_px}px gap"
        );
    }

    #[test]
    fn unit_ink_extents_cover_drawn_pixels() {
        use super::text::GlyphCache;
        let mut semi = GlyphCache::new(FONT_SEMI);
        for unit in ["km/h", "/km", "bpm", "km", "W"] {
            let px = 24.0f32;
            let (_, ink_w) = semi.measure_extents(unit, px, false, 0.0);
            let mut pm = Pixmap::new(256, 64).unwrap();
            semi.draw(&mut pm, unit, 0.0, 48.0, px, [1.0, 1.0, 1.0, 1.0], false, 0.0);
            let mut max_x = 0u32;
            let data = pm.data();
            for y in 0..pm.height() {
                for x in 0..pm.width() {
                    if data[((y * pm.width() + x) * 4 + 3) as usize] > 0 {
                        max_x = max_x.max(x);
                    }
                }
            }
            assert!(
                ink_w + 1.0 >= max_x as f32,
                "{unit}: measured ink {ink_w} < drawn max_x {max_x}"
            );
        }
    }

    #[test]
    fn formats_duration() {
        assert_eq!(fmt_duration(0.0), "00:00");
        assert_eq!(fmt_duration(65.0), "01:05");
        assert_eq!(fmt_duration(3600.0), "1:00:00");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
    }

    #[test]
    fn formats_pace() {
        assert_eq!(fmt_pace(1000.0 / 292.0), "4:52");
        assert_eq!(fmt_pace(0.1), "--:--"); // standing still
        assert_eq!(fmt_pace(10.0), "1:40");
    }

    #[test]
    fn formats_distance() {
        assert_eq!(fmt_distance_km(8420.0), "8.42");
        assert_eq!(fmt_distance_km(15500.0), "15.5");
        assert_eq!(fmt_distance_km(123400.0), "123");
    }

    #[test]
    fn formats_thousands() {
        assert_eq!(fmt_thousands(1624), "1,624");
        assert_eq!(fmt_thousands(842), "842");
        assert_eq!(fmt_thousands(-1234567), "-1,234,567");
    }

    fn synth(sport: SportKind) -> crate::fit::Timeline {
        use crate::fit::Sample;
        use chrono::TimeZone;
        let n = 1800usize; // 30 minutes at 1 Hz
        let gps = sport != SportKind::IndoorRun;
        let mut dist = 0.0f64;
        let mut samples: Vec<Sample> = (0..n)
            .map(|i| {
                let t = i as f64;
                let a = t / n as f64 * std::f64::consts::TAU;
                let speed = match sport {
                    SportKind::BikeRide => 8.5 + 1.5 * (a * 5.0).sin(),
                    SportKind::Hike => 1.4 + 0.2 * (a * 7.0).sin(),
                    _ => 3.3 + 0.4 * (a * 5.0).sin(),
                };
                dist += speed;
                Sample {
                    t,
                    lat: gps.then(|| 47.0 + 0.004 * (a.sin() + 0.3 * (3.0 * a).sin())),
                    lon: gps.then(|| 8.0 + 0.004 * (a.cos() + 0.3 * (2.0 * a).cos())),
                    hr: Some(140.0 + 20.0 * (a * 3.0).sin()),
                    speed: Some(speed),
                    speed_smooth: Some(speed),
                    dist: Some(dist),
                    alt: Some(520.0 + 150.0 * (a / 2.0).sin() + 8.0 * (a * 9.0).sin()),
                    cadence: Some(if sport == SportKind::BikeRide { 92.0 } else { 88.0 }),
                    power: (sport == SportKind::BikeRide).then_some(245.0),
                    ascent: 0.0,
                }
            })
            .collect();
        let mut gain = 0.0;
        for i in 1..n {
            let d = samples[i].alt.unwrap() - samples[i - 1].alt.unwrap();
            if d > 0.0 {
                gain += d;
            }
            samples[i].ascent = gain;
        }
        crate::fit::Timeline {
            start_utc: chrono::Utc.with_ymd_and_hms(2026, 6, 7, 15, 9, 53).unwrap(),
            sport,
            utc_offset_secs: Some(7200),
            samples,
            pauses: vec![],
            has_gps: gps,
        }
    }

    fn render_preview_with_overrides(
        tl: &crate::fit::Timeline,
        w: u32,
        h: u32,
        path: &str,
        overrides: &crate::layout::LayoutOverrides,
    ) {
        let mut r = OverlayRenderer::new(
            tl,
            w,
            h,
            190.0,
            &LayoutConfig::resolve(tl, overrides, 190.0),
        )
        .unwrap();
        let t = tl.duration() * 0.62;
        let snap = tl.snapshot(t);
        let mut out = vec![0u8; (w * h * 4) as usize];
        r.render_frame(&snap, t, 1.0, &mut out);

        let mut bg = Pixmap::new(w, h).unwrap();
        bg.fill(Color::from_rgba8(70, 74, 80, 255));
        let data = bg.data_mut();
        for (i, px) in out.chunks_exact(4).enumerate() {
            let a = px[3] as f32 / 255.0;
            if a == 0.0 {
                continue;
            }
            let d = &mut data[i * 4..i * 4 + 4];
            for c in 0..3 {
                d[c] = (px[c] as f32 * a + d[c] as f32 * (1.0 - a)) as u8;
            }
        }
        bg.save_png(path).unwrap();
    }

    /// Visual smoke test: renders one frame per sport layout to
    /// target/previews/*.png over a grey backdrop. Run with:
    /// `cargo test render_layout_previews -- --ignored`
    #[test]
    #[ignore]
    fn render_layout_previews() {
        std::fs::create_dir_all("target/previews").unwrap();
        let overrides = crate::layout::LayoutOverrides::default();

        for (sport, name) in [
            (SportKind::OutdoorRun, "outdoor-run"),
            (SportKind::IndoorRun, "indoor-run"),
            (SportKind::BikeRide, "bike-ride"),
            (SportKind::Hike, "hike"),
        ] {
            let tl = synth(sport);
            render_preview_with_overrides(&tl, 1080, 1920, &format!("target/previews/{name}.png"), &overrides);
            render_preview_with_overrides(
                &tl,
                2752,
                1530,
                &format!("target/previews/{name}-landscape.png"),
                &overrides,
            );
        }
    }

    /// Generate custom layout previews for documentation.
    #[test]
    #[ignore]
    fn render_custom_layout_previews() {
        use crate::layout::{LayoutOverrides, MetricId, WidgetId};
        std::fs::create_dir_all("target/previews").unwrap();

        // 1. Outdoor run: custom metrics
        let tl = synth(SportKind::OutdoorRun);
        render_preview_with_overrides(
            &tl,
            1080,
            1920,
            "target/previews/style-outdoor-run-custom-metrics.png",
            &LayoutOverrides {
                metrics: Some(vec![MetricId::Distance, MetricId::Pace, MetricId::HeartRate]),
                ..Default::default()
            },
        );

        // 2. Outdoor run: with HR zones
        render_preview_with_overrides(
            &tl,
            1080,
            1920,
            "target/previews/style-outdoor-run-hr-zones.png",
            &LayoutOverrides {
                enable_widgets: vec![WidgetId::HrZones],
                ..Default::default()
            },
        );

        // 3. Bike: with elevation
        let tl_bike = synth(SportKind::BikeRide);
        render_preview_with_overrides(
            &tl_bike,
            1080,
            1920,
            "target/previews/style-bike-elevation.png",
            &LayoutOverrides {
                enable_widgets: vec![WidgetId::Elevation],
                ..Default::default()
            },
        );

        // 4. Hike: minimal
        let tl_hike = synth(SportKind::Hike);
        render_preview_with_overrides(
            &tl_hike,
            1080,
            1920,
            "target/previews/style-hike-minimal.png",
            &LayoutOverrides {
                widgets: Some(crate::layout::WidgetSet {
                    time_chip: true,
                    metrics_panel: true,
                    map: false,
                    elevation: false,
                    hr_zones: false,
                }),
                metrics: Some(vec![MetricId::Distance, MetricId::Altitude]),
                ..Default::default()
            },
        );

        // 5. Edge case: metrics off, elevation + HR zones on
        render_preview_with_overrides(
            &tl_bike,
            1080,
            1920,
            "target/previews/edge-case-no-metrics.png",
            &LayoutOverrides {
                widgets: Some(crate::layout::WidgetSet {
                    time_chip: true,
                    metrics_panel: false,
                    map: true,
                    elevation: true,
                    hr_zones: true,
                }),
                ..Default::default()
            },
        );
    }

    #[test]
    fn demultiply_roundtrip() {
        let mut pm = Pixmap::new(2, 1).unwrap();
        // Half-transparent red via a fill.
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba(1.0, 0.0, 0.0, 0.5).unwrap());
        let mut pb = PathBuilder::new();
        pb.push_circle(0.0, 0.0, 10.0);
        pm.fill_path(
            &pb.finish().unwrap(),
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let mut out = vec![0u8; 8];
        write_rgba_straight(&pm, 1.0, &mut out);
        // Straight red with ~50% alpha.
        assert!(out[0] > 240, "r={}", out[0]);
        assert!((out[3] as i32 - 127).abs() < 5, "a={}", out[3]);
        // Fade halves alpha but keeps color.
        write_rgba_straight(&pm, 0.5, &mut out);
        assert!(out[0] > 240);
        assert!((out[3] as i32 - 63).abs() < 5, "a={}", out[3]);
    }
}
