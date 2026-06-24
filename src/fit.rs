//! FIT activity decoding into a normalized, interpolatable `Timeline`.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use fitparser::profile::MesgNum;
use fitparser::{FitDataRecord, Value};
use std::fs::File;
use std::path::Path;

const SEMICIRCLE_TO_DEG: f64 = 180.0 / 2147483648.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportKind {
    OutdoorRun,
    IndoorRun,
    BikeRide,
    Hike,
}

impl SportKind {
    pub fn label(&self) -> &'static str {
        match self {
            SportKind::OutdoorRun => "outdoor run",
            SportKind::IndoorRun => "indoor run",
            SportKind::BikeRide => "bike ride",
            SportKind::Hike => "hike",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Sample {
    /// Seconds since activity start.
    pub t: f64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub hr: Option<f64>,
    pub speed: Option<f64>,
    /// Smoothed speed in m/s (filled in post-processing).
    pub speed_smooth: Option<f64>,
    pub dist: Option<f64>,
    pub alt: Option<f64>,
    pub cadence: Option<f64>,
    pub power: Option<f64>,
    /// Cumulative elevation gain in meters up to this sample.
    pub ascent: f64,
}

/// Interpolated state of the activity at an arbitrary time.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub hr: Option<f64>,
    pub speed: Option<f64>,
    pub dist: Option<f64>,
    pub alt: Option<f64>,
    pub cadence: Option<f64>,
    pub power: Option<f64>,
    pub ascent: f64,
    /// Moving time (pauses excluded) in seconds.
    pub moving_secs: f64,
    pub paused: bool,
}

#[derive(Debug)]
pub struct Timeline {
    pub start_utc: DateTime<Utc>,
    pub sport: SportKind,
    /// UTC offset of the recording device, from the FIT activity message.
    pub utc_offset_secs: Option<i64>,
    pub samples: Vec<Sample>,
    /// Pause spans as (start, end) seconds since activity start.
    pub pauses: Vec<(f64, f64)>,
    pub has_gps: bool,
}

impl Timeline {
    pub fn duration(&self) -> f64 {
        self.samples.last().map(|s| s.t).unwrap_or(0.0)
    }

    pub fn has_field(&self, f: impl Fn(&Sample) -> bool) -> bool {
        self.samples.iter().any(f)
    }

    /// Moving time at `t`: elapsed time minus completed pause time.
    pub fn moving_time(&self, t: f64) -> f64 {
        let mut paused = 0.0;
        for &(p0, p1) in &self.pauses {
            if t >= p1 {
                paused += p1 - p0;
            } else if t > p0 {
                paused += t - p0;
            }
        }
        (t - paused).max(0.0)
    }

    fn pause_at(&self, t: f64) -> Option<(f64, f64)> {
        self.pauses.iter().copied().find(|&(p0, p1)| t >= p0 && t < p1)
    }

    /// Interpolated snapshot at `t` seconds since activity start.
    /// During pauses values freeze at the pause start.
    pub fn snapshot(&self, t: f64) -> Snapshot {
        let pause = self.pause_at(t);
        let t_eval = pause.map(|(p0, _)| p0).unwrap_or(t);
        let mut snap = self.snapshot_raw(t_eval);
        snap.moving_secs = self.moving_time(t);
        snap.paused = pause.is_some();
        snap
    }

    fn snapshot_raw(&self, t: f64) -> Snapshot {
        let n = self.samples.len();
        if n == 0 {
            return Snapshot::default();
        }
        let t = t.clamp(self.samples[0].t, self.samples[n - 1].t);
        // Index of last sample with sample.t <= t.
        let i = match self
            .samples
            .binary_search_by(|s| s.t.partial_cmp(&t).unwrap())
        {
            Ok(i) => i,
            Err(ins) => ins.saturating_sub(1),
        };
        let a = &self.samples[i];
        let b = self.samples.get(i + 1).unwrap_or(a);
        let span = b.t - a.t;
        // Don't interpolate across big gaps (signal loss / smart recording holes).
        let f = if span > 0.0 && span <= 30.0 {
            ((t - a.t) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Hold the last known value when a side is missing — forward-fill at
        // decode time covers most cases; this handles any remaining holes.
        let lerp = |x: Option<f64>, y: Option<f64>| -> Option<f64> {
            match (x, y) {
                (Some(x), Some(y)) => Some(x + (y - x) * f),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }
        };

        Snapshot {
            hr: lerp(a.hr, b.hr),
            speed: lerp(a.speed_smooth.or(a.speed), b.speed_smooth.or(b.speed)),
            dist: lerp(a.dist, b.dist),
            alt: lerp(a.alt, b.alt),
            cadence: lerp(a.cadence, b.cadence),
            power: lerp(a.power, b.power),
            ascent: a.ascent + (b.ascent - a.ascent) * f,
            moving_secs: 0.0,
            paused: false,
        }
    }
}

fn field<'a>(rec: &'a FitDataRecord, name: &str) -> Option<&'a Value> {
    rec.fields().iter().find(|f| f.name() == name).map(|f| f.value())
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::SInt8(x) => Some(*x as f64),
        Value::UInt8(x) | Value::UInt8z(x) | Value::Byte(x) | Value::Enum(x) => Some(*x as f64),
        Value::SInt16(x) => Some(*x as f64),
        Value::UInt16(x) | Value::UInt16z(x) => Some(*x as f64),
        Value::SInt32(x) => Some(*x as f64),
        Value::UInt32(x) | Value::UInt32z(x) => Some(*x as f64),
        Value::SInt64(x) => Some(*x as f64),
        Value::UInt64(x) | Value::UInt64z(x) => Some(*x as f64),
        Value::Float32(x) => Some(*x as f64),
        Value::Float64(x) => Some(*x),
        _ => None,
    }
}

fn num_field(rec: &FitDataRecord, name: &str) -> Option<f64> {
    field(rec, name).and_then(as_f64)
}

fn str_field<'a>(rec: &'a FitDataRecord, name: &str) -> Option<&'a str> {
    match field(rec, name) {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn utc_field(rec: &FitDataRecord, name: &str) -> Option<DateTime<Utc>> {
    match field(rec, name) {
        Some(Value::Timestamp(dt)) => Some(dt.with_timezone(&Utc)),
        _ => None,
    }
}

fn fit_ref_naive() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1989, 12, 31)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

/// fitparser decodes `local_date_time` values by anchoring the FIT reference
/// date in the *machine's* local timezone. Recover the device's naive local
/// wall-clock time, which is machine-timezone independent.
fn fit_local_naive(dt: &DateTime<chrono::Local>) -> NaiveDateTime {
    let ref_local = chrono::Local
        .from_local_datetime(&fit_ref_naive())
        .single()
        .unwrap_or_else(|| chrono::Local.from_utc_datetime(&fit_ref_naive()));
    let secs = dt.timestamp() - ref_local.timestamp();
    fit_ref_naive() + Duration::seconds(secs)
}

fn detect_sport(sport: Option<&str>, sub_sport: Option<&str>, has_gps: bool) -> SportKind {
    let sub = sub_sport.unwrap_or("");
    match sport.unwrap_or("") {
        "running" => {
            if matches!(sub, "treadmill" | "indoor_running" | "virtual_activity") || !has_gps {
                SportKind::IndoorRun
            } else {
                SportKind::OutdoorRun
            }
        }
        "cycling" | "e_biking" => SportKind::BikeRide,
        "hiking" | "walking" | "mountaineering" => SportKind::Hike,
        _ => {
            if has_gps {
                SportKind::OutdoorRun
            } else {
                SportKind::IndoorRun
            }
        }
    }
}

pub fn decode(path: &Path) -> Result<Timeline> {
    let mut fp = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let records = fitparser::from_reader(&mut fp)
        .map_err(|e| anyhow::anyhow!("decoding FIT file {}: {e}", path.display()))?;
    build_timeline(&records)
}

fn build_timeline(records: &[FitDataRecord]) -> Result<Timeline> {
    let mut samples: Vec<Sample> = Vec::new();
    let mut start_utc: Option<DateTime<Utc>> = None;
    let mut sport: Option<String> = None;
    let mut sub_sport: Option<String> = None;
    let mut utc_offset_secs: Option<i64> = None;
    // (t, is_start) timer events
    let mut timer_events: Vec<(DateTime<Utc>, bool)> = Vec::new();

    for rec in records {
        match rec.kind() {
            MesgNum::Record => {
                let Some(ts) = utc_field(rec, "timestamp") else { continue };
                let start = *start_utc.get_or_insert(ts);
                let t = (ts - start).num_milliseconds() as f64 / 1000.0;
                let lat = num_field(rec, "position_lat").map(|v| v * SEMICIRCLE_TO_DEG);
                let lon = num_field(rec, "position_long").map(|v| v * SEMICIRCLE_TO_DEG);
                let mut cadence = num_field(rec, "cadence");
                if let (Some(c), Some(fc)) = (cadence, num_field(rec, "fractional_cadence")) {
                    cadence = Some(c + fc);
                }
                samples.push(Sample {
                    t,
                    lat,
                    lon,
                    hr: num_field(rec, "heart_rate").filter(|&h| h > 0.0),
                    speed: num_field(rec, "enhanced_speed").or_else(|| num_field(rec, "speed")),
                    speed_smooth: None,
                    dist: num_field(rec, "distance"),
                    alt: num_field(rec, "enhanced_altitude")
                        .or_else(|| num_field(rec, "altitude")),
                    cadence,
                    power: num_field(rec, "power"),
                    ascent: 0.0,
                });
            }
            MesgNum::Session => {
                if sport.is_none() {
                    sport = str_field(rec, "sport").map(str::to_owned);
                    sub_sport = str_field(rec, "sub_sport").map(str::to_owned);
                }
            }
            MesgNum::Event => {
                let is_timer = str_field(rec, "event") == Some("timer");
                if !is_timer {
                    continue;
                }
                let Some(ts) = utc_field(rec, "timestamp") else { continue };
                match str_field(rec, "event_type") {
                    Some("start") => timer_events.push((ts, true)),
                    Some("stop" | "stop_all" | "stop_disable" | "stop_disable_all") => {
                        timer_events.push((ts, false))
                    }
                    _ => {}
                }
            }
            MesgNum::Activity if utc_offset_secs.is_none() => {
                let local = match field(rec, "local_timestamp") {
                    Some(Value::Timestamp(dt)) => Some(fit_local_naive(dt)),
                    _ => None,
                };
                let ts = utc_field(rec, "timestamp");
                if let (Some(local), Some(ts)) = (local, ts) {
                    let offset = (local - ts.naive_utc()).num_seconds();
                    // Sanity bound: ±18h, rounded to nearest minute.
                    if offset.abs() <= 18 * 3600 {
                        utc_offset_secs = Some((offset as f64 / 60.0).round() as i64 * 60);
                    }
                }
            }
            _ => {}
        }
    }

    if samples.is_empty() {
        bail!("FIT file contains no record messages (not an activity file?)");
    }
    samples.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    let start_utc = start_utc.unwrap();

    // Garmin smart recording often omits unchanged fields from individual
    // records (e.g. a GPS-only point with no HR/distance). Carry the last
    // known value forward so snapshots don't flicker to "--".
    forward_fill_sparse(&mut samples);

    let has_gps = samples.iter().any(|s| s.lat.is_some() && s.lon.is_some());
    let sport = detect_sport(sport.as_deref(), sub_sport.as_deref(), has_gps);

    smooth_speed(&mut samples);
    accumulate_ascent(&mut samples);

    // Build pause spans from timer stop -> next start.
    let mut pauses: Vec<(f64, f64)> = Vec::new();
    let mut stop_at: Option<f64> = None;
    timer_events.sort_by_key(|(ts, _)| *ts);
    for (ts, is_start) in timer_events {
        let t = (ts - start_utc).num_milliseconds() as f64 / 1000.0;
        if is_start {
            if let Some(s) = stop_at.take() {
                if t > s + 0.5 {
                    pauses.push((s, t));
                }
            }
        } else if !is_start && stop_at.is_none() {
            stop_at = Some(t);
        }
    }

    Ok(Timeline {
        start_utc,
        sport,
        utc_offset_secs,
        samples,
        pauses,
        has_gps,
    })
}

/// Copy the most recent non-null scalar fields into later sparse records.
fn forward_fill_sparse(samples: &mut [Sample]) {
    for i in 1..samples.len() {
        let prev = samples[i - 1].clone();
        let cur = &mut samples[i];
        if cur.hr.is_none() {
            cur.hr = prev.hr;
        }
        if cur.speed.is_none() {
            cur.speed = prev.speed;
        }
        if cur.dist.is_none() {
            cur.dist = prev.dist;
        }
        if cur.alt.is_none() {
            cur.alt = prev.alt;
        }
        if cur.cadence.is_none() {
            cur.cadence = prev.cadence;
        }
        if cur.power.is_none() {
            cur.power = prev.power;
        }
    }
}

/// Centered moving average over ~5 samples for display-friendly speed.
fn smooth_speed(samples: &mut [Sample]) {
    let speeds: Vec<Option<f64>> = samples.iter().map(|s| s.speed).collect();
    let n = speeds.len();
    for i in 0..n {
        if speeds[i].is_none() {
            continue;
        }
        let lo = i.saturating_sub(2);
        let hi = (i + 2).min(n - 1);
        let mut sum = 0.0;
        let mut cnt = 0usize;
        for v in speeds[lo..=hi].iter().flatten() {
            sum += v;
            cnt += 1;
        }
        samples[i].speed_smooth = Some(sum / cnt as f64);
    }
}

/// Cumulative elevation gain using a lightly smoothed altitude series and a
/// small positive-delta threshold to suppress barometric noise.
fn accumulate_ascent(samples: &mut [Sample]) {
    let alts: Vec<Option<f64>> = samples.iter().map(|s| s.alt).collect();
    let n = alts.len();
    let smoothed: Vec<Option<f64>> = (0..n)
        .map(|i| {
            alts[i]?;
            let lo = i.saturating_sub(3);
            let hi = (i + 3).min(n - 1);
            let vals: Vec<f64> = alts[lo..=hi].iter().flatten().copied().collect();
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        })
        .collect();

    let mut gain = 0.0;
    let mut anchor: Option<f64> = None;
    for i in 0..n {
        if let Some(alt) = smoothed[i] {
            match anchor {
                None => anchor = Some(alt),
                Some(a) => {
                    let d = alt - a;
                    if d >= 0.5 {
                        gain += d;
                        anchor = Some(alt);
                    } else if d < 0.0 {
                        anchor = Some(alt);
                    }
                }
            }
        }
        samples[i].ascent = gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_samples(specs: &[(f64, f64)]) -> Vec<Sample> {
        // (t, speed)
        specs
            .iter()
            .map(|&(t, sp)| Sample {
                t,
                speed: Some(sp),
                speed_smooth: Some(sp),
                hr: Some(100.0 + t),
                dist: Some(t * sp),
                ..Default::default()
            })
            .collect()
    }

    fn mk_timeline(samples: Vec<Sample>, pauses: Vec<(f64, f64)>) -> Timeline {
        Timeline {
            start_utc: Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport: SportKind::OutdoorRun,
            utc_offset_secs: Some(7200),
            samples,
            pauses,
            has_gps: false,
        }
    }

    #[test]
    fn interpolates_between_samples() {
        let tl = mk_timeline(mk_samples(&[(0.0, 2.0), (10.0, 4.0)]), vec![]);
        let s = tl.snapshot(5.0);
        assert!((s.speed.unwrap() - 3.0).abs() < 1e-9);
        assert!((s.hr.unwrap() - 105.0).abs() < 1e-9);
    }

    #[test]
    fn does_not_interpolate_across_large_gaps() {
        let tl = mk_timeline(mk_samples(&[(0.0, 2.0), (120.0, 6.0)]), vec![]);
        let s = tl.snapshot(60.0);
        assert!((s.speed.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn clamps_outside_range() {
        let tl = mk_timeline(mk_samples(&[(0.0, 2.0), (10.0, 4.0)]), vec![]);
        assert!((tl.snapshot(-5.0).speed.unwrap() - 2.0).abs() < 1e-9);
        assert!((tl.snapshot(50.0).speed.unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn moving_time_excludes_pauses() {
        let tl = mk_timeline(
            mk_samples(&[(0.0, 2.0), (100.0, 2.0)]),
            vec![(10.0, 20.0), (50.0, 60.0)],
        );
        assert!((tl.moving_time(5.0) - 5.0).abs() < 1e-9);
        assert!((tl.moving_time(15.0) - 10.0).abs() < 1e-9); // frozen mid-pause
        assert!((tl.moving_time(30.0) - 20.0).abs() < 1e-9);
        assert!((tl.moving_time(100.0) - 80.0).abs() < 1e-9);
        let s = tl.snapshot(15.0);
        assert!(s.paused);
    }

    #[test]
    fn forward_fill_keeps_hr_and_distance_through_sparse_records() {
        let mut samples = vec![
            Sample {
                t: 0.0,
                hr: Some(120.0),
                dist: Some(0.0),
                speed: Some(3.0),
                ..Default::default()
            },
            Sample {
                t: 5.0,
                lat: Some(47.0),
                lon: Some(8.0),
                ..Default::default()
            },
            Sample {
                t: 10.0,
                hr: Some(125.0),
                dist: Some(50.0),
                speed: Some(3.2),
                ..Default::default()
            },
        ];
        forward_fill_sparse(&mut samples);
        assert_eq!(samples[1].hr, Some(120.0));
        assert_eq!(samples[1].dist, Some(0.0));
        assert_eq!(samples[1].speed, Some(3.0));

        let tl = Timeline {
            start_utc: Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport: SportKind::OutdoorRun,
            utc_offset_secs: None,
            samples,
            pauses: vec![],
            has_gps: true,
        };
        let snap = tl.snapshot(5.0);
        assert_eq!(snap.hr, Some(120.0));
        assert_eq!(snap.dist, Some(0.0));
    }

    #[test]
    fn ascent_accumulates_with_threshold() {
        let mut samples: Vec<Sample> = (0..20)
            .map(|i| Sample {
                t: i as f64,
                alt: Some(100.0 + i as f64), // steady 1 m/s climb
                ..Default::default()
            })
            .collect();
        accumulate_ascent(&mut samples);
        let total = samples.last().unwrap().ascent;
        assert!(total > 10.0, "expected meaningful gain, got {total}");
    }
}
