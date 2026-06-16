//! Layout configuration: sport defaults, CLI overrides, and data-aware resolution.

use crate::fit::{SportKind, Timeline};
use std::fmt;

/// User-facing metric identifiers (CLI tokens map to these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricId {
    Pace,
    Speed,
    HeartRate,
    Distance,
    Cadence,
    Power,
    ElevGain,
    Altitude,
}

/// Overlay widgets (distinct from individual metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetId {
    TimeChip,
    MetricsPanel,
    Map,
    Elevation,
    HrZones,
}

/// Resolved widget enable flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WidgetSet {
    pub time_chip: bool,
    pub metrics_panel: bool,
    pub map: bool,
    pub elevation: bool,
    pub hr_zones: bool,
}

/// Raw user intent before resolution against activity data.
#[derive(Debug, Clone, Default)]
pub struct LayoutOverrides {
    pub metrics: Option<Vec<MetricId>>,
    pub widgets: Option<WidgetSet>,
    pub disable_widgets: Vec<WidgetId>,
    pub enable_widgets: Vec<WidgetId>,
}

/// Fully resolved layout for one activity.
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub metrics: Vec<MetricId>,
    pub widgets: WidgetSet,
    pub warnings: Vec<String>,
}

/// Parse failure for metric or widget tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub token: String,
    pub kind: ParseKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseKind {
    Metric,
    Widget,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let valid = match self.kind {
            ParseKind::Metric => VALID_METRIC_TOKENS.join(", "),
            ParseKind::Widget => VALID_WIDGET_TOKENS.join(", "),
        };
        write!(
            f,
            "unknown {} '{}'; valid: {}",
            match self.kind {
                ParseKind::Metric => "metric",
                ParseKind::Widget => "widget",
            },
            self.token,
            valid
        )
    }
}

impl std::error::Error for ParseError {}

const VALID_METRIC_TOKENS: &[&str] = &[
    "pace",
    "speed",
    "hr",
    "heart-rate",
    "distance",
    "dist",
    "cadence",
    "power",
    "elev-gain",
    "elevation-gain",
    "gain",
    "altitude",
    "alt",
    "elevation",
];

const VALID_WIDGET_TOKENS: &[&str] = &[
    "time",
    "time-chip",
    "metrics",
    "metrics-panel",
    "map",
    "noodle",
    "elevation",
    "elev-profile",
    "hr-zones",
    "zones",
];

impl WidgetSet {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn enable(&mut self, id: WidgetId) {
        match id {
            WidgetId::TimeChip => self.time_chip = true,
            WidgetId::MetricsPanel => self.metrics_panel = true,
            WidgetId::Map => self.map = true,
            WidgetId::Elevation => self.elevation = true,
            WidgetId::HrZones => self.hr_zones = true,
        }
    }

    pub fn disable(&mut self, id: WidgetId) {
        match id {
            WidgetId::TimeChip => self.time_chip = false,
            WidgetId::MetricsPanel => self.metrics_panel = false,
            WidgetId::Map => self.map = false,
            WidgetId::Elevation => self.elevation = false,
            WidgetId::HrZones => self.hr_zones = false,
        }
    }

    pub fn wants(&self, id: WidgetId) -> bool {
        match id {
            WidgetId::TimeChip => self.time_chip,
            WidgetId::MetricsPanel => self.metrics_panel,
            WidgetId::Map => self.map,
            WidgetId::Elevation => self.elevation,
            WidgetId::HrZones => self.hr_zones,
        }
    }
}

impl MetricId {
    pub fn parse_token(token: &str) -> Result<Self, ParseError> {
        match token.trim().to_ascii_lowercase().as_str() {
            "pace" => Ok(MetricId::Pace),
            "speed" => Ok(MetricId::Speed),
            "hr" | "heart-rate" => Ok(MetricId::HeartRate),
            "distance" | "dist" => Ok(MetricId::Distance),
            "cadence" => Ok(MetricId::Cadence),
            "power" => Ok(MetricId::Power),
            "elev-gain" | "elevation-gain" | "gain" => Ok(MetricId::ElevGain),
            "altitude" | "alt" | "elevation" => Ok(MetricId::Altitude),
            _ => Err(ParseError {
                token: token.to_string(),
                kind: ParseKind::Metric,
            }),
        }
    }

    /// Parse a comma-separated metric list. Empty input is an error.
    pub fn parse_list(s: &str) -> Result<Vec<MetricId>, ParseError> {
        let tokens: Vec<&str> = s.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return Err(ParseError {
                token: String::new(),
                kind: ParseKind::Metric,
            });
        }
        tokens.iter().map(|t| Self::parse_token(t)).collect()
    }

    pub fn label(&self) -> &'static str {
        match self {
            MetricId::Pace => "pace",
            MetricId::Speed => "speed",
            MetricId::HeartRate => "hr",
            MetricId::Distance => "distance",
            MetricId::Cadence => "cadence",
            MetricId::Power => "power",
            MetricId::ElevGain => "elev-gain",
            MetricId::Altitude => "altitude",
        }
    }
}

impl WidgetId {
    pub fn parse_token(token: &str) -> Result<Self, ParseError> {
        match token.trim().to_ascii_lowercase().as_str() {
            "time" | "time-chip" => Ok(WidgetId::TimeChip),
            "metrics" | "metrics-panel" => Ok(WidgetId::MetricsPanel),
            "map" | "noodle" => Ok(WidgetId::Map),
            "elevation" | "elev-profile" => Ok(WidgetId::Elevation),
            "hr-zones" | "zones" => Ok(WidgetId::HrZones),
            _ => Err(ParseError {
                token: token.to_string(),
                kind: ParseKind::Widget,
            }),
        }
    }

    /// Parse a comma-separated widget list into a set. Empty input is an error.
    pub fn parse_list(s: &str) -> Result<WidgetSet, ParseError> {
        let tokens: Vec<&str> = s.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return Err(ParseError {
                token: String::new(),
                kind: ParseKind::Widget,
            });
        }
        let mut set = WidgetSet::none();
        for token in tokens {
            set.enable(WidgetId::parse_token(token)?);
        }
        Ok(set)
    }

    pub fn label(&self) -> &'static str {
        match self {
            WidgetId::TimeChip => "time",
            WidgetId::MetricsPanel => "metrics",
            WidgetId::Map => "map",
            WidgetId::Elevation => "elevation",
            WidgetId::HrZones => "hr-zones",
        }
    }
}

impl LayoutConfig {
    /// Sport defaults + overrides + timeline data → final layout.
    pub fn resolve(tl: &Timeline, overrides: &LayoutOverrides, _max_hr: f64) -> Self {
        let mut warnings = Vec::new();

        let metric_source = overrides
            .metrics
            .clone()
            .unwrap_or_else(|| Self::default_metrics_for(tl.sport));
        let (metrics, metric_warnings) = filter_metrics(tl, &metric_source);
        warnings.extend(metric_warnings);

        let widgets = apply_widget_overrides(tl.sport, overrides, &mut warnings);
        let (mut widgets, widget_warnings) = filter_widgets(tl, widgets);
        warnings.extend(widget_warnings);

        if widgets.metrics_panel && metrics.is_empty() {
            widgets.metrics_panel = false;
            warnings.push("no metrics to display; metrics panel hidden".to_string());
        }

        Self {
            metrics,
            widgets,
            warnings,
        }
    }

    pub fn default_metrics_for(sport: SportKind) -> Vec<MetricId> {
        match sport {
            SportKind::OutdoorRun | SportKind::IndoorRun => {
                vec![
                    MetricId::Pace,
                    MetricId::HeartRate,
                    MetricId::Distance,
                    MetricId::Cadence,
                ]
            }
            SportKind::BikeRide => vec![
                MetricId::Speed,
                MetricId::Power,
                MetricId::HeartRate,
                MetricId::Distance,
            ],
            SportKind::Hike => vec![
                MetricId::Distance,
                MetricId::ElevGain,
                MetricId::HeartRate,
                MetricId::Altitude,
            ],
        }
    }

    pub fn default_widgets_for(sport: SportKind) -> WidgetSet {
        WidgetSet {
            time_chip: true,
            metrics_panel: true,
            map: matches!(
                sport,
                SportKind::OutdoorRun | SportKind::BikeRide | SportKind::Hike
            ),
            elevation: sport == SportKind::Hike,
            hr_zones: sport == SportKind::IndoorRun,
        }
    }
}

fn apply_widget_overrides(
    sport: SportKind,
    overrides: &LayoutOverrides,
    warnings: &mut Vec<String>,
) -> WidgetSet {
    if let Some(set) = overrides.widgets {
        if !overrides.enable_widgets.is_empty() || !overrides.disable_widgets.is_empty() {
            warnings.push(
                "--widgets replaces sport defaults; --widget/--no-widget flags ignored".to_string(),
            );
        }
        return set;
    }

    let mut widgets = LayoutConfig::default_widgets_for(sport);
    for id in &overrides.enable_widgets {
        widgets.enable(*id);
    }
    for id in &overrides.disable_widgets {
        widgets.disable(*id);
    }
    widgets
}

fn filter_metrics(tl: &Timeline, source: &[MetricId]) -> (Vec<MetricId>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut metrics = Vec::new();

    for &metric in source {
        if !seen.insert(metric) {
            warnings.push(format!("duplicate metric '{}' ignored", metric.label()));
            continue;
        }
        if metric_has_data(tl, metric) {
            metrics.push(metric);
        } else {
            warnings.push(format!(
                "metric '{}' omitted (no data in FIT file)",
                metric.label()
            ));
        }
    }

    (metrics, warnings)
}

fn filter_widgets(tl: &Timeline, mut widgets: WidgetSet) -> (WidgetSet, Vec<String>) {
    let mut warnings = Vec::new();

    for id in [
        WidgetId::Map,
        WidgetId::Elevation,
        WidgetId::HrZones,
    ] {
        if widgets.wants(id) && !widget_data_available(tl, id) {
            widgets.disable(id);
            warnings.push(format!(
                "widget '{}' omitted ({})",
                id.label(),
                widget_omit_reason(id)
            ));
        }
    }

    (widgets, warnings)
}

pub fn metric_has_data(tl: &Timeline, metric: MetricId) -> bool {
    match metric {
        MetricId::Pace | MetricId::Speed => tl.has_field(|s| s.speed.is_some()),
        MetricId::HeartRate => tl.has_field(|s| s.hr.is_some()),
        MetricId::Distance => tl.has_field(|s| s.dist.is_some()),
        MetricId::Cadence => tl.has_field(|s| s.cadence.map(|c| c > 0.0).unwrap_or(false)),
        MetricId::Power => tl.has_field(|s| s.power.is_some()),
        MetricId::ElevGain | MetricId::Altitude => tl.has_field(|s| s.alt.is_some()),
    }
}

pub fn widget_data_available(tl: &Timeline, widget: WidgetId) -> bool {
    match widget {
        WidgetId::TimeChip | WidgetId::MetricsPanel => true,
        WidgetId::Map => tl.has_gps,
        WidgetId::Elevation => tl.has_field(|s| s.alt.is_some()),
        WidgetId::HrZones => tl.has_field(|s| s.hr.is_some()),
    }
}

fn widget_omit_reason(widget: WidgetId) -> &'static str {
    match widget {
        WidgetId::Map => "no GPS in FIT file",
        WidgetId::Elevation => "no altitude in FIT file",
        WidgetId::HrZones => "no heart rate in FIT file",
        WidgetId::TimeChip | WidgetId::MetricsPanel => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::{Sample, Timeline};
    use chrono::TimeZone;

    fn mk_timeline(sport: SportKind, samples: Vec<Sample>, has_gps: bool) -> Timeline {
        Timeline {
            start_utc: chrono::Utc.with_ymd_and_hms(2026, 6, 7, 15, 0, 0).unwrap(),
            sport,
            utc_offset_secs: Some(7200),
            samples,
            pauses: vec![],
            has_gps,
        }
    }

    fn full_sample() -> Sample {
        Sample {
            t: 0.0,
            hr: Some(125.0),
            dist: Some(9510.0),
            speed: Some(3.0),
            power: Some(245.0),
            cadence: Some(88.0),
            lat: Some(47.0),
            lon: Some(8.0),
            alt: Some(520.0),
            ..Default::default()
        }
    }

    #[test]
    fn default_metrics_match_sport() {
        assert_eq!(
            LayoutConfig::default_metrics_for(SportKind::OutdoorRun),
            vec![
                MetricId::Pace,
                MetricId::HeartRate,
                MetricId::Distance,
                MetricId::Cadence,
            ]
        );
        assert_eq!(
            LayoutConfig::default_metrics_for(SportKind::BikeRide),
            vec![
                MetricId::Speed,
                MetricId::Power,
                MetricId::HeartRate,
                MetricId::Distance,
            ]
        );
        assert_eq!(
            LayoutConfig::default_metrics_for(SportKind::Hike),
            vec![
                MetricId::Distance,
                MetricId::ElevGain,
                MetricId::HeartRate,
                MetricId::Altitude,
            ]
        );
    }

    #[test]
    fn default_widgets_match_sport() {
        assert_eq!(
            LayoutConfig::default_widgets_for(SportKind::OutdoorRun),
            WidgetSet {
                time_chip: true,
                metrics_panel: true,
                map: true,
                elevation: false,
                hr_zones: false,
            }
        );
        assert_eq!(
            LayoutConfig::default_widgets_for(SportKind::IndoorRun),
            WidgetSet {
                time_chip: true,
                metrics_panel: true,
                map: false,
                elevation: false,
                hr_zones: true,
            }
        );
        assert_eq!(
            LayoutConfig::default_widgets_for(SportKind::Hike),
            WidgetSet {
                time_chip: true,
                metrics_panel: true,
                map: true,
                elevation: true,
                hr_zones: false,
            }
        );
    }

    #[test]
    fn parse_metrics_case_insensitive() {
        let ids = MetricId::parse_list("Pace,HR,Distance").unwrap();
        assert_eq!(
            ids,
            vec![MetricId::Pace, MetricId::HeartRate, MetricId::Distance]
        );
    }

    #[test]
    fn parse_metrics_rejects_unknown() {
        let err = MetricId::parse_token("heartrate").unwrap_err();
        assert_eq!(err.kind, ParseKind::Metric);
        assert!(err.to_string().contains("unknown metric"));
        assert!(err.to_string().contains("valid:"));
    }

    #[test]
    fn parse_metrics_dedupes_on_resolve() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let overrides = LayoutOverrides {
            metrics: Some(vec![MetricId::Pace, MetricId::Pace, MetricId::HeartRate]),
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert_eq!(layout.metrics, vec![MetricId::Pace, MetricId::HeartRate]);
        assert!(layout
            .warnings
            .iter()
            .any(|w| w.contains("duplicate metric 'pace' ignored")));
    }

    #[test]
    fn metrics_override_reorders() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let overrides = LayoutOverrides {
            metrics: Some(vec![
                MetricId::Pace,
                MetricId::HeartRate,
                MetricId::Distance,
            ]),
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert_eq!(
            layout.metrics,
            vec![MetricId::Pace, MetricId::HeartRate, MetricId::Distance]
        );
    }

    #[test]
    fn metrics_override_filters_missing_data() {
        let sample = Sample {
            power: None,
            ..full_sample()
        };
        let tl = mk_timeline(SportKind::BikeRide, vec![sample], true);
        let layout = LayoutConfig::resolve(&tl, &LayoutOverrides::default(), 190.0);
        assert!(!layout.metrics.contains(&MetricId::Power));
        assert_eq!(
            layout.metrics,
            vec![
                MetricId::Speed,
                MetricId::HeartRate,
                MetricId::Distance,
            ]
        );
    }

    #[test]
    fn widget_enable_adds_hr_zones_outdoor() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let overrides = LayoutOverrides {
            enable_widgets: vec![WidgetId::HrZones],
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert!(layout.widgets.hr_zones);
        assert!(layout.widgets.map);
    }

    #[test]
    fn widget_disable_removes_map() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let overrides = LayoutOverrides {
            disable_widgets: vec![WidgetId::Map],
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert!(!layout.widgets.map);
        assert!(layout.widgets.time_chip);
    }

    #[test]
    fn widgets_flag_replaces_defaults() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let overrides = LayoutOverrides {
            widgets: Some(WidgetSet {
                time_chip: true,
                metrics_panel: true,
                map: false,
                elevation: false,
                hr_zones: false,
            }),
            enable_widgets: vec![WidgetId::Map],
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert!(!layout.widgets.map);
        assert!(layout
            .warnings
            .iter()
            .any(|w| w.contains("--widgets replaces sport defaults")));
    }

    #[test]
    fn resolve_empty_metrics_hides_panel() {
        let sample = Sample {
            hr: None,
            dist: None,
            speed: None,
            cadence: None,
            ..Default::default()
        };
        let tl = mk_timeline(SportKind::OutdoorRun, vec![sample], false);
        let overrides = LayoutOverrides {
            metrics: Some(vec![MetricId::HeartRate, MetricId::Pace]),
            ..Default::default()
        };
        let layout = LayoutConfig::resolve(&tl, &overrides, 190.0);
        assert!(layout.metrics.is_empty());
        assert!(!layout.widgets.metrics_panel);
        assert!(layout
            .warnings
            .iter()
            .any(|w| w.contains("no metrics to display")));
    }

    #[test]
    fn resolve_defaults_match_outdoor_run_with_full_data() {
        let tl = mk_timeline(SportKind::OutdoorRun, vec![full_sample()], true);
        let layout = LayoutConfig::resolve(&tl, &LayoutOverrides::default(), 190.0);
        assert_eq!(
            layout.metrics,
            vec![
                MetricId::Pace,
                MetricId::HeartRate,
                MetricId::Distance,
                MetricId::Cadence,
            ]
        );
        assert_eq!(
            layout.widgets,
            WidgetSet {
                time_chip: true,
                metrics_panel: true,
                map: true,
                elevation: false,
                hr_zones: false,
            }
        );
        assert!(layout.warnings.is_empty());
    }

    #[test]
    fn resolve_indoor_run_hr_zones_need_hr_data() {
        let sample = Sample {
            lat: None,
            lon: None,
            hr: None,
            ..full_sample()
        };
        let tl = mk_timeline(SportKind::IndoorRun, vec![sample], false);
        let layout = LayoutConfig::resolve(&tl, &LayoutOverrides::default(), 190.0);
        assert!(!layout.widgets.hr_zones);
        assert!(!layout.widgets.map);
        assert!(layout
            .warnings
            .iter()
            .any(|w| w.contains("widget 'hr-zones' omitted")));
    }

    #[test]
    fn resolve_hike_elevation_needs_altitude() {
        let sample = Sample {
            alt: None,
            ..full_sample()
        };
        let tl = mk_timeline(SportKind::Hike, vec![sample], true);
        let layout = LayoutConfig::resolve(&tl, &LayoutOverrides::default(), 190.0);
        assert!(!layout.widgets.elevation);
        assert!(!layout.metrics.contains(&MetricId::ElevGain));
        assert!(!layout.metrics.contains(&MetricId::Altitude));
    }

    #[test]
    fn parse_widgets_from_list() {
        let set = WidgetId::parse_list("time,map,metrics").unwrap();
        assert!(set.time_chip);
        assert!(set.map);
        assert!(set.metrics_panel);
        assert!(!set.elevation);
    }

    #[test]
    fn metric_has_data_respects_cadence_threshold() {
        let tl = mk_timeline(
            SportKind::OutdoorRun,
            vec![Sample {
                cadence: Some(0.0),
                ..full_sample()
            }],
            true,
        );
        assert!(!metric_has_data(&tl, MetricId::Cadence));
    }
}
