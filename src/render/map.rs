//! GPS track projection ("noodle map") and generic time-indexed polylines.

use crate::fit::Sample;

/// A polyline indexed by activity time, projected into widget-local pixels.
#[derive(Debug)]
pub struct Track {
    pub ts: Vec<f64>,
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
}

impl Track {
    /// Project GPS samples into a `w` x `h` box with `pad` padding,
    /// preserving aspect ratio (equirectangular with latitude correction).
    pub fn from_gps(samples: &[Sample], w: f32, h: f32, pad: f32) -> Option<Track> {
        let pts: Vec<(f64, f64, f64)> = samples
            .iter()
            .filter_map(|s| Some((s.t, s.lat?, s.lon?)))
            .collect();
        if pts.len() < 2 {
            return None;
        }
        let mid_lat = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
        let k = mid_lat.to_radians().cos().max(0.01);
        let raw: Vec<(f64, f64, f64)> = pts
            .iter()
            .map(|&(t, lat, lon)| (t, lon * k, -lat))
            .collect();
        Some(fit_into_box(&raw, w, h, pad))
    }

    /// Build an elevation profile polyline: x = time, y = altitude.
    pub fn elevation_profile(samples: &[Sample], w: f32, h: f32, pad: f32) -> Option<Track> {
        let pts: Vec<(f64, f64, f64)> = samples
            .iter()
            .filter_map(|s| Some((s.t, s.t, s.alt?)))
            .collect();
        if pts.len() < 2 {
            return None;
        }
        let raw: Vec<(f64, f64, f64)> = pts.iter().map(|&(t, x, alt)| (t, x, -alt)).collect();
        Some(fit_into_box_stretch(&raw, w, h, pad))
    }

    /// Index of the last point with `ts <= t`.
    pub fn index_at(&self, t: f64) -> usize {
        match self.ts.binary_search_by(|v| v.partial_cmp(&t).unwrap()) {
            Ok(i) => i,
            Err(ins) => ins.saturating_sub(1),
        }
    }

    /// Interpolated position at time `t` (clamped to the track range).
    pub fn point_at(&self, t: f64) -> (f32, f32) {
        let n = self.ts.len();
        if t <= self.ts[0] {
            return (self.xs[0], self.ys[0]);
        }
        if t >= self.ts[n - 1] {
            return (self.xs[n - 1], self.ys[n - 1]);
        }
        let i = self.index_at(t);
        let j = (i + 1).min(n - 1);
        let span = self.ts[j] - self.ts[i];
        let f = if span > 0.0 && span <= 30.0 {
            ((t - self.ts[i]) / span).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        (
            self.xs[i] + (self.xs[j] - self.xs[i]) * f,
            self.ys[i] + (self.ys[j] - self.ys[i]) * f,
        )
    }
}

/// Fit raw (t, x, y) points into the box, preserving aspect ratio, centered.
fn fit_into_box(raw: &[(f64, f64, f64)], w: f32, h: f32, pad: f32) -> Track {
    let (min_x, max_x, min_y, max_y) = bounds(raw);
    let span_x = (max_x - min_x).max(1e-9);
    let span_y = (max_y - min_y).max(1e-9);
    let scale = ((w - 2.0 * pad) as f64 / span_x).min((h - 2.0 * pad) as f64 / span_y);
    let ox = (w as f64 - span_x * scale) / 2.0;
    let oy = (h as f64 - span_y * scale) / 2.0;
    project(raw, min_x, min_y, scale, scale, ox, oy)
}

/// Fit raw points into the box stretching both axes independently.
fn fit_into_box_stretch(raw: &[(f64, f64, f64)], w: f32, h: f32, pad: f32) -> Track {
    let (min_x, max_x, min_y, max_y) = bounds(raw);
    let span_x = (max_x - min_x).max(1e-9);
    let span_y = (max_y - min_y).max(1e-9);
    let sx = (w - 2.0 * pad) as f64 / span_x;
    let sy = (h - 2.0 * pad) as f64 / span_y;
    project(raw, min_x, min_y, sx, sy, pad as f64, pad as f64)
}

fn bounds(raw: &[(f64, f64, f64)]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &(_, x, y) in raw {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, max_x, min_y, max_y)
}

fn project(
    raw: &[(f64, f64, f64)],
    min_x: f64,
    min_y: f64,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
) -> Track {
    let mut ts = Vec::with_capacity(raw.len());
    let mut xs = Vec::with_capacity(raw.len());
    let mut ys = Vec::with_capacity(raw.len());
    for &(t, x, y) in raw {
        ts.push(t);
        xs.push(((x - min_x) * sx + ox) as f32);
        ys.push(((y - min_y) * sy + oy) as f32);
    }
    Track { ts, xs, ys }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::Sample;

    fn gps_samples() -> Vec<Sample> {
        (0..=10)
            .map(|i| Sample {
                t: i as f64,
                lat: Some(47.0 + i as f64 * 0.001),
                lon: Some(8.0 + i as f64 * 0.001),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn projection_fits_inside_box() {
        let track = Track::from_gps(&gps_samples(), 300.0, 300.0, 20.0).unwrap();
        for (&x, &y) in track.xs.iter().zip(&track.ys) {
            assert!((0.0..=300.0).contains(&x), "x={x}");
            assert!((0.0..=300.0).contains(&y), "y={y}");
        }
        assert!(track.ys.last().unwrap() < track.ys.first().unwrap());
        assert!(track.xs.last().unwrap() > track.xs.first().unwrap());
    }

    #[test]
    fn projection_preserves_aspect() {
        let samples: Vec<Sample> = (0..=10)
            .map(|i| Sample {
                t: i as f64,
                lat: Some(0.0 + i as f64 * 0.001),
                lon: Some(0.0 + i as f64 * 0.002),
                ..Default::default()
            })
            .collect();
        let track = Track::from_gps(&samples, 300.0, 300.0, 0.0).unwrap();
        let w = track.xs.iter().fold(f32::MIN, |a, &b| a.max(b))
            - track.xs.iter().fold(f32::MAX, |a, &b| a.min(b));
        let h = track.ys.iter().fold(f32::MIN, |a, &b| a.max(b))
            - track.ys.iter().fold(f32::MAX, |a, &b| a.min(b));
        assert!((w / h - 2.0).abs() < 0.05, "aspect was {}", w / h);
    }

    #[test]
    fn point_at_interpolates_and_clamps() {
        let track = Track::from_gps(&gps_samples(), 300.0, 300.0, 20.0).unwrap();
        let (x0, y0) = track.point_at(-1.0);
        assert_eq!((x0, y0), (track.xs[0], track.ys[0]));
        let (xn, yn) = track.point_at(99.0);
        assert_eq!(
            (xn, yn),
            (*track.xs.last().unwrap(), *track.ys.last().unwrap())
        );
        let (xm, _) = track.point_at(4.5);
        assert!(xm > track.xs[4] && xm < track.xs[5]);
    }

    #[test]
    fn requires_two_gps_points() {
        let samples = vec![Sample {
            t: 0.0,
            lat: Some(47.0),
            lon: Some(8.0),
            ..Default::default()
        }];
        assert!(Track::from_gps(&samples, 300.0, 300.0, 20.0).is_none());
    }
}
