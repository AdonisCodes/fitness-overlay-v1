//! Visual theme: colors and UI scale (stored in settings / presets).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThemeConfig {
    pub accent: [u8; 3],
    pub panel_bg: [u8; 4],
    pub zone_colors: [[u8; 3]; 5],
    /// Multiplier on the base HUD scale (1.0 = default).
    pub ui_scale: f32,
    pub label_alpha: f32,
    pub unit_alpha: f32,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: [255, 122, 26],
            panel_bg: [10, 12, 18, 115],
            zone_colors: [
                [132, 138, 148],
                [66, 133, 244],
                [52, 168, 83],
                [255, 122, 26],
                [217, 48, 37],
            ],
            ui_scale: 1.0,
            label_alpha: 0.72,
            unit_alpha: 0.68,
        }
    }
}

impl ThemeConfig {
    pub fn accent_rgb(&self) -> (u8, u8, u8) {
        (self.accent[0], self.accent[1], self.accent[2])
    }

    pub fn panel_bg_rgba(&self) -> (u8, u8, u8, u8) {
        (
            self.panel_bg[0],
            self.panel_bg[1],
            self.panel_bg[2],
            self.panel_bg[3],
        )
    }

    pub fn label_color(&self) -> [f32; 4] {
        [1.0, 1.0, 1.0, self.label_alpha]
    }

    pub fn unit_color(&self) -> [f32; 4] {
        [1.0, 1.0, 1.0, self.unit_alpha]
    }

    /// High-contrast blue accent preset.
    pub fn cool() -> Self {
        Self {
            accent: [66, 133, 244],
            zone_colors: [
                [100, 110, 125],
                [66, 133, 244],
                [52, 168, 83],
                [255, 193, 7],
                [217, 48, 37],
            ],
            ..Self::default()
        }
    }

    /// Compact HUD for dense metric rows.
    pub fn compact() -> Self {
        Self {
            ui_scale: 0.88,
            ..Self::default()
        }
    }
}
