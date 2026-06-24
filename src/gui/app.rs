//! fitnessoverlay editor window.

use crate::cli::fade_at;
use crate::config::{
    load_preset, save_preset, AppSettings, LayoutPresetData, Preset, list_presets,
};
use crate::fit::{self, Timeline};
use crate::layout::{LayoutConfig, MetricId, WidgetSet};
use crate::preview::{extract_video_frame, render_preview_composite};
use crate::render::OverlayRenderer;
use crate::theme::ThemeConfig;
use crate::video::{self, SyncMap, VideoInfo};
use anyhow::Result;
use chrono::Offset;
use eframe::egui;
use std::path::PathBuf;

pub fn run_gui() -> Result<()> {
    let settings = AppSettings::load().unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([1024.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "fitnessoverlay",
        options,
        Box::new(|cc| Ok(Box::new(EditorApp::new(cc, settings)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

struct EditorApp {
    settings: AppSettings,
    fit_path: Option<PathBuf>,
    video_paths: Vec<PathBuf>,
    timeline: Option<Timeline>,
    video: Option<VideoInfo>,
    sync: Option<SyncMap>,
    utc_offset_secs: i64,
    preview_time: f64,
    preview_rgb: Vec<u8>,
    preview_size: (u32, u32),
    preview_texture: Option<egui::TextureHandle>,
    preset_names: Vec<String>,
    draft_preset_name: String,
    layout_draft: LayoutPresetData,
    theme_draft: ThemeConfig,
    warnings: Vec<String>,
    status: String,
    needs_preview_refresh: bool,
}

impl EditorApp {
    fn new(_cc: &eframe::CreationContext<'_>, settings: AppSettings) -> Self {
        let preset_names = list_presets().unwrap_or_default();
        let theme_draft = settings.active_theme();
        let layout_draft = settings
            .layout
            .clone()
            .unwrap_or_else(|| LayoutPresetData::from_overrides(&settings.active_layout_overrides()));
        Self {
            settings,
            fit_path: None,
            video_paths: Vec::new(),
            timeline: None,
            video: None,
            sync: None,
            utc_offset_secs: 0,
            preview_time: 0.0,
            preview_rgb: Vec::new(),
            preview_size: (0, 0),
            preview_texture: None,
            preset_names,
            draft_preset_name: String::new(),
            layout_draft,
            theme_draft,
            warnings: Vec::new(),
            status: "Open a .fit file and a video to preview the overlay.".into(),
            needs_preview_refresh: false,
        }
    }

    fn reload_presets(&mut self) {
        self.preset_names = list_presets().unwrap_or_default();
    }

    fn load_fit(&mut self, path: PathBuf) -> Result<()> {
        let timeline = fit::decode(&path)?;
        self.utc_offset_secs = timeline.utc_offset_secs.unwrap_or_else(|| {
            timeline
                .start_utc
                .with_timezone(&chrono::Local)
                .offset()
                .fix()
                .local_minus_utc() as i64
        });
        self.fit_path = Some(path);
        self.timeline = Some(timeline);
        self.recompute_sync();
        self.needs_preview_refresh = true;
        Ok(())
    }

    fn add_video(&mut self, path: PathBuf) -> Result<()> {
        let info = video::probe(&path)?;
        if !self.video_paths.iter().any(|p| p == &path) {
            self.video_paths.push(path);
        }
        self.video = Some(info);
        self.recompute_sync();
        self.needs_preview_refresh = true;
        Ok(())
    }

    fn recompute_sync(&mut self) {
        self.warnings.clear();
        let (Some(tl), Some(vid)) = (&self.timeline, &self.video) else {
            self.sync = None;
            return;
        };
        self.sync = Some(video::compute_sync(
            vid.start_local,
            vid.duration,
            self.utc_offset_secs,
            tl.start_utc,
            tl.duration(),
            self.settings.sync_offset,
        ));
        if self.sync.as_ref().and_then(|s| s.visible).is_none() {
            self.warnings
                .push("Video does not overlap the activity timeline.".into());
        }
    }

    fn visible_range(&self) -> Option<(f64, f64)> {
        self.sync.as_ref().and_then(|s| s.visible)
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let (Some(tl), Some(vid), Some(sync)) = (&self.timeline, &self.video, &self.sync) else {
            return;
        };
        let Some((lo, hi)) = sync.visible else {
            self.status = "No overlap — adjust sync offset or pick another clip.".into();
            return;
        };
        let t_v = self.preview_time.clamp(lo, hi);
        self.preview_time = t_v;

        let path = &vid.path;
        match extract_video_frame(path, t_v, 720) {
            Ok((mut rgb, w, h)) => {
                let mut warnings = Vec::new();
                let overrides = self.layout_draft.to_overrides();
                let layout = LayoutConfig::resolve(tl, &overrides, self.settings.max_hr);
                warnings.extend(layout.warnings.clone());
                self.warnings = warnings;

                if let Ok(mut renderer) =
                    OverlayRenderer::new(tl, w, h, self.settings.max_hr, &layout, &self.theme_draft)
                {
                    let t_act = t_v + sync.offset;
                    let snap = tl.snapshot(t_act);
                    let fade = fade_at(t_v, lo, hi, vid.duration);
                    render_preview_composite(&mut renderer, &snap, t_act, fade, &mut rgb, w, h);
                }

                self.preview_size = (w, h);
                self.preview_rgb = rgb;
                let image = egui::ColorImage::from_rgb([w as usize, h as usize], &self.preview_rgb);
                self.preview_texture = Some(ctx.load_texture(
                    "preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                self.status = format!(
                    "Preview @ {} (activity {})",
                    crate::render::fmt_duration(t_v),
                    crate::render::fmt_duration(t_v + sync.offset)
                );
            }
            Err(e) => self.status = format!("Preview error: {e}"),
        }
        self.needs_preview_refresh = false;
    }

    fn apply_preset(&mut self, name: &str) {
        if let Ok(p) = load_preset(name) {
            self.layout_draft = p.layout;
            self.theme_draft = p.theme;
            self.settings.active_preset = Some(name.to_string());
            let _ = self.settings.save();
            self.needs_preview_refresh = true;
        }
    }

    fn save_current_preset(&mut self) -> Result<()> {
        let name = if self.draft_preset_name.trim().is_empty() {
            format!("preset-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
        } else {
            self.draft_preset_name.trim().to_string()
        };
        let preset = Preset {
            name: name.clone(),
            layout: self.layout_draft.clone(),
            theme: self.theme_draft.clone(),
        };
        save_preset(&preset)?;
        self.settings.active_preset = Some(name);
        self.settings.layout = Some(self.layout_draft.clone());
        self.settings.theme = self.theme_draft.clone();
        self.settings.save()?;
        self.reload_presets();
        self.draft_preset_name.clear();
        self.status = format!("Saved preset '{}'", preset.name);
        Ok(())
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.needs_preview_refresh {
            self.refresh_preview(ctx);
        }

        egui::SidePanel::left("controls")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.heading("fitnessoverlay");
                ui.label("Burn Garmin HUDs onto Insta360 video.");
                ui.separator();

                ui.label("Activity (.fit)");
                ui.horizontal(|ui| {
                    if ui.button("Open .fit…").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("FIT", &["fit"])
                            .pick_file()
                        {
                            if let Err(e) = self.load_fit(p) {
                                self.status = format!("{e}");
                            }
                        }
                    }
                    if let Some(p) = &self.fit_path {
                        ui.label(p.file_name().unwrap_or_default().to_string_lossy());
                    }
                });

                ui.add_space(8.0);
                ui.label("Videos");
                if ui.button("Add video…").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Video", &["mp4", "MP4", "mov"])
                        .pick_file()
                    {
                        if let Err(e) = self.add_video(p) {
                            self.status = format!("{e}");
                        }
                    }
                }
                for p in &self.video_paths {
                    ui.label(format!("• {}", p.file_name().unwrap_or_default().to_string_lossy()));
                }

                ui.separator();
                ui.label("Presets");
                egui::ComboBox::from_label("Load")
                    .selected_text(self.settings.active_preset.as_deref().unwrap_or("none"))
                    .show_ui(ui, |ui| {
                        for name in &self.preset_names.clone() {
                            if ui.selectable_label(false, name).clicked() {
                                self.apply_preset(name);
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.draft_preset_name);
                    if ui.button("Save preset").clicked() {
                        if let Err(e) = self.save_current_preset() {
                            self.status = format!("{e}");
                        }
                    }
                });

                ui.separator();
                ui.collapsing("Metrics", |ui| {
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Pace, "Pace");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Speed, "Speed");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::HeartRate, "HR");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Distance, "Distance");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Cadence, "Cadence");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Power, "Power");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::ElevGain, "Elev gain");
                    metric_toggle(ui, &mut self.layout_draft, MetricId::Altitude, "Altitude");
                });

                ui.collapsing("Widgets", |ui| {
                    let w = self.layout_draft.widgets.get_or_insert_with(WidgetSet::none);
                    ui.checkbox(&mut w.time_chip, "Timer");
                    ui.checkbox(&mut w.metrics_panel, "Metrics panel");
                    ui.checkbox(&mut w.map, "Map");
                    ui.checkbox(&mut w.elevation, "Elevation");
                    ui.checkbox(&mut w.hr_zones, "HR zones");
                });

                ui.collapsing("Theme", |ui| {
                    ui.add(egui::Slider::new(&mut self.theme_draft.ui_scale, 0.7..=1.2).text("UI scale"));
                    let mut accent_rgb = [
                        self.theme_draft.accent[0] as f32 / 255.0,
                        self.theme_draft.accent[1] as f32 / 255.0,
                        self.theme_draft.accent[2] as f32 / 255.0,
                    ];
                    ui.color_edit_button_rgb(&mut accent_rgb);
                    self.theme_draft.accent = [
                        (accent_rgb[0] * 255.0).round() as u8,
                        (accent_rgb[1] * 255.0).round() as u8,
                        (accent_rgb[2] * 255.0).round() as u8,
                    ];
                    ui.label("Accent colour");
                    ui.add(egui::Slider::new(&mut self.theme_draft.label_alpha, 0.4..=1.0).text("Label opacity"));
                    ui.add(egui::Slider::new(&mut self.theme_draft.unit_alpha, 0.4..=1.0).text("Unit opacity"));
                });

                ui.separator();
                ui.add(egui::Slider::new(&mut self.settings.sync_offset, -30.0..=30.0).text("Sync offset (s)"));
                ui.add(egui::Slider::new(&mut self.settings.max_hr, 120.0..=220.0).text("Max HR"));

                if ui.button("Apply & refresh preview").clicked() {
                    self.recompute_sync();
                    self.needs_preview_refresh = true;
                }
                if ui.button("Save settings").clicked() {
                    self.settings.layout = Some(self.layout_draft.clone());
                    self.settings.theme = self.theme_draft.clone();
                    if let Err(e) = self.settings.save() {
                        self.status = format!("{e}");
                    }
                }

                for w in &self.warnings {
                    ui.colored_label(egui::Color32::YELLOW, w);
                }
                ui.separator();
                ui.label(&self.status);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Preview (pre-export)");
            if let Some((lo, hi)) = self.visible_range() {
                let resp = ui.add(egui::Slider::new(&mut self.preview_time, lo..=hi).text("Video time"));
                if resp.changed() {
                    self.needs_preview_refresh = true;
                }
                ui.label(format!(
                    "Activity time: {}",
                    crate::render::fmt_duration(self.preview_time + self.sync.as_ref().map(|s| s.offset).unwrap_or(0.0))
                ));
            }

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.available_height() - 40.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    if let Some(tex) = &self.preview_texture {
                        let (w, h) = self.preview_size;
                        let max_w = ui.available_width();
                        let max_h = ui.available_height();
                        let scale = (max_w / w as f32).min(max_h / h as f32);
                        let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                        ui.image((tex.id(), size));
                    } else {
                        ui.label("Load a .fit file and video to see the composited preview.");
                    }
                },
            );
        });

        if self.needs_preview_refresh {
            ctx.request_repaint();
        }
    }
}

fn metric_toggle(ui: &mut egui::Ui, layout: &mut LayoutPresetData, id: MetricId, label: &str) {
    let metrics = layout.metrics.get_or_insert_with(Vec::new);
    let mut on = metrics.contains(&id);
    if ui.checkbox(&mut on, label).changed() {
        if on {
            if !metrics.contains(&id) {
                metrics.push(id);
            }
        } else {
            metrics.retain(|m| *m != id);
        }
    }
}