//! Application settings and layout presets on disk.

use crate::layout::{LayoutOverrides, MetricId, WidgetId, WidgetSet};
use crate::theme::ThemeConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_DIR: &str = "fitnessoverlay";

/// Serializable layout overrides for settings / presets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LayoutPresetData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Vec<MetricId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widgets: Option<WidgetSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enable_widgets: Vec<WidgetId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable_widgets: Vec<WidgetId>,
}

impl LayoutPresetData {
    pub fn to_overrides(&self) -> LayoutOverrides {
        LayoutOverrides {
            metrics: self.metrics.clone(),
            widgets: self.widgets,
            enable_widgets: self.enable_widgets.clone(),
            disable_widgets: self.disable_widgets.clone(),
        }
    }

    pub fn from_overrides(o: &LayoutOverrides) -> Self {
        Self {
            metrics: o.metrics.clone(),
            widgets: o.widgets,
            enable_widgets: o.enable_widgets.clone(),
            disable_widgets: o.disable_widgets.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub name: String,
    pub layout: LayoutPresetData,
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppSettings {
    pub max_hr: f64,
    pub sync_offset: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset: Option<String>,
    pub out_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset: Option<String>,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutPresetData>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            max_hr: 190.0,
            sync_offset: 0.0,
            utc_offset: None,
            out_dir: PathBuf::from("out"),
            active_preset: None,
            theme: ThemeConfig::default(),
            layout: None,
        }
    }
}

impl AppSettings {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR)
    }

    pub fn settings_path() -> PathBuf {
        Self::config_dir().join("settings.json")
    }

    pub fn presets_dir() -> PathBuf {
        Self::config_dir().join("presets")
    }

    pub fn load() -> Result<Self> {
        let path = Self::settings_path();
        if !path.is_file() {
            let s = Self::default();
            s.save()?;
            s.seed_default_presets()?;
            return Ok(s);
        }
        let data = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(Self::config_dir())?;
        let path = Self::settings_path();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    pub fn active_theme(&self) -> ThemeConfig {
        if let Some(name) = &self.active_preset {
            if let Ok(p) = load_preset(name) {
                return p.theme;
            }
        }
        self.theme.clone()
    }

    pub fn active_layout_overrides(&self) -> LayoutOverrides {
        if let Some(name) = &self.active_preset {
            if let Ok(p) = load_preset(name) {
                return p.layout.to_overrides();
            }
        }
        self.layout
            .as_ref()
            .map(LayoutPresetData::to_overrides)
            .unwrap_or_default()
    }

    fn seed_default_presets(&self) -> Result<()> {
        let dir = Self::presets_dir();
        if dir.is_dir() && fs::read_dir(&dir)?.next().is_some() {
            return Ok(());
        }
        fs::create_dir_all(&dir)?;
        for p in default_presets() {
            save_preset(&p)?;
        }
        Ok(())
    }
}

pub fn load_preset(name: &str) -> Result<Preset> {
    let path = preset_path(name);
    let data = fs::read_to_string(&path).with_context(|| format!("reading preset {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("parsing preset {}", path.display()))
}

pub fn save_preset(preset: &Preset) -> Result<()> {
    fs::create_dir_all(AppSettings::presets_dir())?;
    let path = preset_path(&preset.name);
    let json = serde_json::to_string_pretty(preset)?;
    fs::write(&path, json).with_context(|| format!("writing preset {}", path.display()))
}

pub fn list_presets() -> Result<Vec<String>> {
    let dir = AppSettings::presets_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

pub fn delete_preset(name: &str) -> Result<()> {
    let path = preset_path(name);
    if path.is_file() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

fn preset_path(name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    AppSettings::presets_dir().join(format!("{safe}.json"))
}

pub fn default_presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "sport-default".into(),
            layout: LayoutPresetData::default(),
            theme: ThemeConfig::default(),
        },
        Preset {
            name: "distance-pace-hr".into(),
            layout: LayoutPresetData {
                metrics: Some(vec![MetricId::Distance, MetricId::Pace, MetricId::HeartRate]),
                ..Default::default()
            },
            theme: ThemeConfig::default(),
        },
        Preset {
            name: "bike-power".into(),
            layout: LayoutPresetData {
                metrics: Some(vec![
                    MetricId::Speed,
                    MetricId::Power,
                    MetricId::HeartRate,
                    MetricId::Distance,
                ]),
                ..Default::default()
            },
            theme: ThemeConfig::default(),
        },
        Preset {
            name: "hike-minimal".into(),
            layout: LayoutPresetData {
                metrics: Some(vec![MetricId::Distance, MetricId::Altitude]),
                widgets: Some(WidgetSet {
                    time_chip: true,
                    metrics_panel: true,
                    map: false,
                    elevation: false,
                    hr_zones: false,
                }),
                ..Default::default()
            },
            theme: ThemeConfig::compact(),
        },
        Preset {
            name: "cool-accent".into(),
            layout: LayoutPresetData::default(),
            theme: ThemeConfig::cool(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_roundtrip() {
        let p = Preset {
            name: "test".into(),
            layout: LayoutPresetData {
                metrics: Some(vec![MetricId::Pace, MetricId::HeartRate]),
                ..Default::default()
            },
            theme: ThemeConfig::cool(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Preset = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
