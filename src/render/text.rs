//! Glyph rasterization cache and text blitting.
//!
//! Glyph coverage masks are rasterized once per (char, size) with `ab_glyph`
//! and afterwards every text draw is a plain alpha blit, which keeps per-frame
//! text rendering cheap.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use std::collections::HashMap;
use tiny_skia::Pixmap;

pub struct CachedGlyph {
    mask: Vec<u8>,
    w: u32,
    h: u32,
    /// Offset of the mask's top-left from the pen position (x) / baseline (y).
    off_x: i32,
    off_y: i32,
    advance: f32,
}

pub struct GlyphCache {
    font: FontRef<'static>,
    glyphs: HashMap<(char, u32), CachedGlyph>,
    digit_advance: HashMap<u32, f32>,
}

impl GlyphCache {
    pub fn new(font_bytes: &'static [u8]) -> Self {
        let font = FontRef::try_from_slice(font_bytes).expect("invalid bundled font");
        GlyphCache {
            font,
            glyphs: HashMap::new(),
            digit_advance: HashMap::new(),
        }
    }

    fn digit_advance(&mut self, px: u32) -> f32 {
        if let Some(&a) = self.digit_advance.get(&px) {
            return a;
        }
        let scaled = self.font.as_scaled(PxScale::from(px as f32));
        let max = ('0'..='9')
            .map(|c| scaled.h_advance(self.font.glyph_id(c)))
            .fold(0.0f32, f32::max);
        self.digit_advance.insert(px, max);
        max
    }

    fn ensure(&mut self, c: char, px: u32) -> &CachedGlyph {
        if !self.glyphs.contains_key(&(c, px)) {
            let scale = PxScale::from(px as f32);
            let scaled = self.font.as_scaled(scale);
            let gid = self.font.glyph_id(c);
            let advance = scaled.h_advance(gid);
            let glyph = gid.with_scale_and_position(scale, point(0.0, 0.0));
            let cached = if let Some(og) = self.font.outline_glyph(glyph) {
                let b = og.px_bounds();
                let w = b.width().ceil().max(0.0) as u32;
                let h = b.height().ceil().max(0.0) as u32;
                let mut mask = vec![0u8; (w * h) as usize];
                og.draw(|x, y, cov| {
                    if x < w && y < h {
                        mask[(y * w + x) as usize] = (cov * 255.0) as u8;
                    }
                });
                CachedGlyph {
                    mask,
                    w,
                    h,
                    off_x: b.min.x.floor() as i32,
                    off_y: b.min.y.floor() as i32,
                    advance,
                }
            } else {
                CachedGlyph {
                    mask: Vec::new(),
                    w: 0,
                    h: 0,
                    off_x: 0,
                    off_y: 0,
                    advance,
                }
            };
            self.glyphs.insert((c, px), cached);
        }
        &self.glyphs[&(c, px)]
    }

    pub fn measure(&mut self, text: &str, px: f32, tabular: bool, tracking: f32) -> f32 {
        let pxk = px.round() as u32;
        let tab = if tabular { self.digit_advance(pxk) } else { 0.0 };
        let mut w = 0.0f32;
        for c in text.chars() {
            let adv = if tabular && c.is_ascii_digit() {
                tab
            } else {
                self.ensure(c, pxk).advance
            };
            w += adv + tracking;
        }
        (w - tracking).max(0.0)
    }

    /// Draw `text` with its baseline at (`x`, `y_baseline`). Returns the width.
    /// `color` is straight RGBA in 0..=1. `tabular` forces uniform digit
    /// advances so numbers don't jitter as values change.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y_baseline: f32,
        px: f32,
        color: [f32; 4],
        tabular: bool,
        tracking: f32,
    ) -> f32 {
        let pxk = px.round() as u32;
        let tab = if tabular { self.digit_advance(pxk) } else { 0.0 };
        let mut pen = x;
        for c in text.chars() {
            let g = self.ensure(c, pxk);
            let (adv, centering) = if tabular && c.is_ascii_digit() {
                (tab, (tab - g.advance) / 2.0)
            } else {
                (g.advance, 0.0)
            };
            blit_mask(pixmap, g, pen + centering, y_baseline, color);
            pen += adv + tracking;
        }
        pen - tracking - x
    }
}

fn blit_mask(pixmap: &mut Pixmap, g: &CachedGlyph, pen_x: f32, baseline_y: f32, color: [f32; 4]) {
    if g.w == 0 || g.h == 0 {
        return;
    }
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let x0 = (pen_x.round() as i32) + g.off_x;
    let y0 = (baseline_y.round() as i32) + g.off_y;
    let data = pixmap.data_mut();
    for row in 0..g.h as i32 {
        let py = y0 + row;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..g.w as i32 {
            let px_ = x0 + col;
            if px_ < 0 || px_ >= pw {
                continue;
            }
            let cov = g.mask[(row as u32 * g.w + col as u32) as usize];
            if cov == 0 {
                continue;
            }
            let a = cov as f32 / 255.0 * color[3];
            if a <= 0.0 {
                continue;
            }
            let idx = ((py * pw + px_) * 4) as usize;
            let inv = 1.0 - a;
            // Premultiplied source-over.
            data[idx] = (color[0] * a * 255.0 + data[idx] as f32 * inv) as u8;
            data[idx + 1] = (color[1] * a * 255.0 + data[idx + 1] as f32 * inv) as u8;
            data[idx + 2] = (color[2] * a * 255.0 + data[idx + 2] as f32 * inv) as u8;
            data[idx + 3] = (a * 255.0 + data[idx + 3] as f32 * inv) as u8;
        }
    }
}
