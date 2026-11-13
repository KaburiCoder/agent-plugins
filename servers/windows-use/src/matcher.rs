//! Multi-scale template matching. A coarse pass runs normalized cross-correlation on
//! downscaled images, then each candidate is refined at full resolution with zero-mean
//! normalized cross-correlation (ZNCC), which is what the returned score means.

use image::imageops::{self, FilterType};
use image::{GrayImage, RgbaImage};
use imageproc::template_matching::{MatchTemplateMethod, match_template_parallel};

const SCALES: [f32; 7] = [1.0, 0.9, 1.1, 0.8, 1.25, 0.67, 1.5];
const PEAKS: usize = 8;
/// The template's shorter side in the coarse pass, about. Smaller is faster (the pass costs the
/// square of the factor less), but a few pixels of text no longer tell places apart.
const COARSE_SIDE: u32 = 7;

#[derive(Debug, Clone, Copy)]
pub struct Match {
    /// Top-left of the match in haystack pixels.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub score: f32,
}

impl Match {
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w as i32 / 2, self.y + self.h as i32 / 2)
    }
}

#[cfg(test)]
pub fn find(haystack: &RgbaImage, template: &RgbaImage) -> Option<Match> {
    candidates(haystack, template, f32::MIN).into_iter().max_by(|a, b| a.score.total_cmp(&b.score))
}

/// Whether the template has enough detail to be located: a flat one matches every flat
/// area equally well.
pub fn usable(template: &RgbaImage) -> bool {
    TemplateStats::new(&imageops::grayscale(template)).is_some()
}

/// Every distinct place scoring at least `min`. Screens often show identical controls
/// (two "취소" buttons), so the caller picks among them by where the target used to be.
pub fn candidates(haystack: &RgbaImage, template: &RgbaImage, min: f32) -> Vec<Match> {
    candidates_until(haystack, template, min, &|| false)
}

/// As [`candidates`], giving up between sizes once `stop` says so (what was found so far is
/// returned, for the caller to throw away).
pub fn candidates_until(haystack: &RgbaImage, template: &RgbaImage, min: f32, stop: &dyn Fn() -> bool) -> Vec<Match> {
    let hay = imageops::grayscale(haystack);
    let tpl = imageops::grayscale(template);
    if TemplateStats::new(&tpl).is_none() {
        return Vec::new();
    }
    let mut found: Vec<Match> = Vec::new();
    for scale in SCALES {
        if stop() {
            break;
        }
        let t = if scale == 1.0 {
            tpl.clone()
        } else {
            let w = (tpl.width() as f32 * scale).round().max(4.0) as u32;
            let h = (tpl.height() as f32 * scale).round().max(4.0) as u32;
            imageops::resize(&tpl, w, h, FilterType::Triangle)
        };
        // As large as the area is fine: one place to look at.
        if t.width() > hay.width() || t.height() > hay.height() {
            continue;
        }
        for m in find_at_scale(&hay, &t).into_iter().filter(|m| m.score >= min) {
            match found.iter_mut().find(|f| overlaps(f, &m)) {
                Some(f) if m.score > f.score => *f = m,
                Some(_) => {}
                None => found.push(m),
            }
        }
        if scale == 1.0 && found.iter().any(|m| m.score >= 0.95) {
            break;
        }
    }
    found
}

fn overlaps(a: &Match, b: &Match) -> bool {
    let (ax, ay) = a.center();
    let (bx, by) = b.center();
    (ax - bx).unsigned_abs() < a.w.min(b.w) / 2 && (ay - by).unsigned_abs() < a.h.min(b.h) / 2
}

/// The best refinement of each coarse peak.
fn find_at_scale(hay: &GrayImage, tpl: &GrayImage) -> Vec<Match> {
    let f = (tpl.width().min(tpl.height()) / COARSE_SIDE).clamp(1, 8);
    let (hs, ts) = if f > 1 {
        (
            imageops::resize(hay, hay.width() / f, hay.height() / f, FilterType::Triangle),
            imageops::resize(tpl, (tpl.width() / f).max(4), (tpl.height() / f).max(4), FilterType::Triangle),
        )
    } else {
        (hay.clone(), tpl.clone())
    };
    if ts.width() > hs.width() || ts.height() > hs.height() {
        return Vec::new();
    }
    let mut scores = match_template_parallel(&hs, &ts, MatchTemplateMethod::CrossCorrelationNormalized);

    let Some(stats) = TemplateStats::new(tpl) else { return Vec::new() };
    let radius = f as i32 + 2;
    let mut out = Vec::new();
    for _ in 0..PEAKS {
        let Some((px, py, _)) = argmax(&scores) else { break };
        suppress(&mut scores, px, py, ts.width() / 2, ts.height() / 2);
        let (cx, cy) = ((px * f) as i32, (py * f) as i32);
        let mut best: Option<Match> = None;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x as u32 + tpl.width() > hay.width() || y as u32 + tpl.height() > hay.height() {
                    continue;
                }
                let s = stats.zncc(hay, x as u32, y as u32);
                if best.is_none_or(|b| s > b.score) {
                    best = Some(Match { x, y, w: tpl.width(), h: tpl.height(), score: s });
                }
            }
        }
        out.extend(best);
    }
    out
}

fn argmax(img: &image::ImageBuffer<image::Luma<f32>, Vec<f32>>) -> Option<(u32, u32, f32)> {
    let mut best: Option<(u32, u32, f32)> = None;
    for (x, y, p) in img.enumerate_pixels() {
        let v = p.0[0];
        if v.is_finite() && best.is_none_or(|b| v > b.2) {
            best = Some((x, y, v));
        }
    }
    best.filter(|b| b.2 > f32::MIN)
}

fn suppress(img: &mut image::ImageBuffer<image::Luma<f32>, Vec<f32>>, cx: u32, cy: u32, rx: u32, ry: u32) {
    let (w, h) = img.dimensions();
    for y in cy.saturating_sub(ry)..(cy + ry + 1).min(h) {
        for x in cx.saturating_sub(rx)..(cx + rx + 1).min(w) {
            img.put_pixel(x, y, image::Luma([f32::MIN]));
        }
    }
}

struct TemplateStats<'a> {
    tpl: &'a GrayImage,
    centered: Vec<f32>,
    /// What the centred values add up to: 0 but for rounding, which a large template makes
    /// count (see zncc).
    centered_sum: f64,
    norm: f32,
}

impl<'a> TemplateStats<'a> {
    fn new(tpl: &'a GrayImage) -> Option<Self> {
        let n = (tpl.width() * tpl.height()) as f64;
        let mean = tpl.pixels().map(|p| p.0[0] as u64).sum::<u64>() as f64 / n;
        let centered: Vec<f32> = tpl.pixels().map(|p| (p.0[0] as f64 - mean) as f32).collect();
        let centered_sum = centered.iter().map(|&v| v as f64).sum::<f64>();
        let norm = centered.iter().map(|&v| v as f64 * v as f64).sum::<f64>().sqrt() as f32;
        (norm > 1e-3 * (n as f32).sqrt() * 4.0).then_some(Self { tpl, centered, centered_sum, norm })
    }

    /// One pass over the place. The template's values are centred, so the place's own mean
    /// drops out of the cross term, all but the share of it the rounding of those values left.
    fn zncc(&self, hay: &GrayImage, x0: u32, y0: u32) -> f32 {
        let (tw, th) = (self.tpl.width() as usize, self.tpl.height() as usize);
        let (hw, raw) = (hay.width() as usize, hay.as_raw());
        let (mut sum, mut sq, mut cross) = (0u64, 0u64, 0f64);
        for y in 0..th {
            let start = (y0 as usize + y) * hw + x0 as usize;
            let row = &raw[start..start + tw];
            for (&v, &c) in row.iter().zip(&self.centered[y * tw..(y + 1) * tw]) {
                sum += v as u64;
                sq += v as u64 * v as u64;
                cross += v as f64 * c as f64;
            }
        }
        let n = (tw * th) as f64;
        let cross = cross - sum as f64 / n * self.centered_sum;
        let var = (sq as f64 - sum as f64 * sum as f64 / n).max(0.0);
        let denom = var.sqrt() * self.norm as f64;
        if denom < 1e-3 { 0.0 } else { (cross / denom) as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A screen-like image: flat panels with a few distinct "buttons".
    fn screen() -> RgbaImage {
        let mut img = RgbaImage::from_pixel(800, 500, Rgba([238, 240, 243, 255]));
        for (i, (x, y)) in [(60u32, 40u32), (260, 40), (460, 40), (300, 300)].into_iter().enumerate() {
            for dy in 0..44 {
                for dx in 0..150 {
                    let edge = dx < 2 || dy < 2 || dx > 147 || dy > 41;
                    let glyph = (dx / 9 + dy / 7 + i as u32 * 3).is_multiple_of(4)
                        && (12..32).contains(&dy)
                        && (20..130).contains(&dx);
                    let v = if edge {
                        120
                    } else if glyph {
                        30
                    } else {
                        255
                    };
                    img.put_pixel(x + dx, y + dy, Rgba([v, v, v, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn finds_exact_crop() {
        let s = screen();
        let tpl = imageops::crop_imm(&s, 255, 35, 160, 54).to_image();
        let m = find(&s, &tpl).expect("match");
        assert_eq!((m.x, m.y), (255, 35));
        assert!(m.score > 0.99, "score {}", m.score);
    }

    /// A picked element as large as its whole window (a one-item drop-down menu) is captured at
    /// the window's size; searching that window must still find it.
    #[test]
    fn finds_template_as_large_as_the_search_area() {
        let s = screen();
        let area = imageops::crop_imm(&s, 255, 35, 160, 54).to_image();
        let m = find(&area, &area).expect("match");
        assert_eq!((m.x, m.y), (0, 0));
        assert!(m.score > 0.99, "score {}", m.score);
    }

    #[test]
    fn finds_scaled_template() {
        let s = screen();
        let crop = imageops::crop_imm(&s, 455, 35, 160, 54).to_image();
        // Captured at 125% and searched at 100%.
        let tpl = imageops::resize(&crop, 200, 68, FilterType::Triangle);
        let m = find(&s, &tpl).expect("match");
        let (cx, cy) = m.center();
        assert!((cx - 535).abs() <= 4 && (cy - 62).abs() <= 4, "center {cx},{cy}");
        assert!(m.score > 0.85, "score {}", m.score);
    }

    /// A form full of small text: thin dark strokes on white, like a medical chart screen.
    fn text_screen() -> RgbaImage {
        let mut img = RgbaImage::from_pixel(1280, 720, Rgba([255, 255, 255, 255]));
        let mut seed = 7u32;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % n
        };
        for _ in 0..900 {
            let (x, y) = (rnd(1260), rnd(700));
            let (w, h) = (2 + rnd(12), 2 + rnd(12));
            let horizontal = rnd(2) == 0;
            for i in 0..if horizontal { w } else { h } {
                let (px, py) = if horizontal { (x + i, y) } else { (x, y + i) };
                img.put_pixel(px, py, Rgba([20, 20, 20, 255]));
            }
        }
        img
    }

    /// Mostly white with a few strokes: plain normalized correlation scores every white
    /// area about as high as the real place, so the real place must still win.
    #[test]
    fn finds_small_text_on_white() {
        let mut s = text_screen();
        let glyph = |x: u32, y: u32| (y % 11 < 2 && x > 3 && x < 19) || (x % 9 < 2 && y > 4 && y < 38 && y % 11 > 3);
        let (gx, gy) = (900, 410);
        for y in 0..44 {
            for x in 0..23 {
                let v = if glyph(x, y) { Rgba([120, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) };
                s.put_pixel(gx + x, gy + y, v);
            }
        }
        let tpl = imageops::crop_imm(&s, gx, gy, 23, 44).to_image();
        let m = find(&s, &tpl).expect("match");
        assert_eq!((m.x, m.y), (gx as i32, gy as i32), "score {}", m.score);
        assert!(m.score > 0.99, "score {}", m.score);
    }

    /// The target is drawn on a tinted background now (selected or hovered), so it is no
    /// longer a pixel-exact copy of what was captured.
    #[test]
    fn finds_tinted_label_among_text() {
        let mut s = text_screen();
        let glyph = |x: u32, y: u32| (y % 9 < 2 && x > 5 && x < 70) || (x % 13 < 2 && y > 4 && y < 26);
        let draw = |s: &mut RgbaImage, gx: u32, gy: u32, bg: u8| {
            for y in 0..30 {
                for x in 0..76 {
                    let v = if glyph(x, y) { 30 } else { bg };
                    s.put_pixel(gx + x, gy + y, Rgba([v, v, v, 255]));
                }
            }
        };
        let mut clean = RgbaImage::from_pixel(76, 30, Rgba([255, 255, 255, 255]));
        draw(&mut clean, 0, 0, 255);
        draw(&mut s, 640, 300, 205);
        let m = find(&s, &clean).expect("match");
        assert_eq!((m.x, m.y), (640, 300), "score {}", m.score);
        assert!(m.score > 0.95, "score {}", m.score);
    }

    /// A large template, all but flat, on a place all but flat: the score stays a correlation.
    #[test]
    fn large_templates_score_at_most_one() {
        let mut tpl = GrayImage::from_pixel(330, 320, image::Luma([251]));
        for i in 0..40 {
            tpl.put_pixel(i * 8, i * 7, image::Luma([3]));
        }
        let mut hay = GrayImage::from_pixel(330, 320, image::Luma([250]));
        hay.put_pixel(5, 5, image::Luma([249]));
        let stats = TemplateStats::new(&tpl).expect("detail enough");
        let s = stats.zncc(&hay, 0, 0);
        assert!(s.abs() <= 1.0001, "score {s}");
        assert!((stats.zncc(&tpl, 0, 0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn missing_template_scores_low() {
        let s = screen();
        let mut tpl = RgbaImage::from_pixel(120, 40, Rgba([255, 255, 255, 255]));
        for x in 0..120 {
            for y in 0..40 {
                if (x * 7 + y * 13) % 11 < 3 {
                    tpl.put_pixel(x, y, Rgba([200, 20, 20, 255]));
                }
            }
        }
        let score = find(&s, &tpl).map(|m| m.score).unwrap_or(0.0);
        assert!(score < 0.85, "score {score}");
    }
}
