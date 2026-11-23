//! Vector strokes stored relative to an exact anchor at their own scale level.

use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};

pub type StrokeId = u64;

/// One input sample in level-local units (1 unit ≈ 1–2 screen px at drawing time).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    /// 0..1
    pub pressure: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub id: StrokeId,
    /// `floor(log2 zoom)` at drawing time. One local unit = `2^-level` world units.
    pub level: i32,
    /// Integer anchor in level units (world = anchor · 2^-level).
    pub anchor: [BigInt; 2],
    pub points: Vec<StrokePoint>,
    /// RGBA, sRGB, straight alpha.
    pub color: [u8; 4],
    /// Full width at pressure 1, in level units.
    pub width: f32,
    /// Draw order (lower = further back). Pieces left by the eraser keep their parent's order.
    pub order: u64,
    #[serde(skip)]
    pub cache: StrokeCache,
}

/// Derived data, rebuilt after loading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeCache {
    /// min x, min y, max x, max y in local units, including the brush radius.
    pub bbox: [f32; 4],
    pub anchor_fast: Option<[i128; 2]>,
}

/// Pressure → radius factor. Keeps a visible minimum so light strokes don't vanish.
pub fn pressure_factor(p: f32) -> f32 {
    0.15 + 0.85 * p.clamp(0.0, 1.0).powf(0.8)
}

impl Stroke {
    pub fn new(id: StrokeId, level: i32, anchor: [BigInt; 2], color: [u8; 4], width: f32) -> Self {
        let mut s = Self { id, level, anchor, points: Vec::new(), color, width, order: id, cache: StrokeCache::default() };
        s.refresh_cache();
        s
    }

    pub fn radius_at(&self, p: &StrokePoint) -> f32 {
        0.5 * self.width * pressure_factor(p.pressure)
    }

    pub fn refresh_cache(&mut self) {
        let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &self.points {
            let r = self.radius_at(p);
            bb[0] = bb[0].min(p.x - r);
            bb[1] = bb[1].min(p.y - r);
            bb[2] = bb[2].max(p.x + r);
            bb[3] = bb[3].max(p.y + r);
        }
        if self.points.is_empty() {
            bb = [0.0; 4];
        }
        self.cache.bbox = bb;
        self.cache.anchor_fast = self.anchor[0].to_i128().zip(self.anchor[1].to_i128()).map(|(a, b)| [a, b]);
    }

    pub fn push(&mut self, p: StrokePoint) {
        self.points.push(p);
        let r = self.radius_at(&p);
        let bb = &mut self.cache.bbox;
        if self.points.len() == 1 {
            *bb = [p.x - r, p.y - r, p.x + r, p.y + r];
        } else {
            bb[0] = bb[0].min(p.x - r);
            bb[1] = bb[1].min(p.y - r);
            bb[2] = bb[2].max(p.x + r);
            bb[3] = bb[3].max(p.y + r);
        }
    }

    /// Erases the part of the stroke touched by a circle of radius `r` at local point `c`.
    ///
    /// A point counts as erased when the circle overlaps the ink there (distance to the centre
    /// line ≤ `r` + local brush radius), so the round caps of the remaining pieces end tangent
    /// to the eraser. Crossings are located exactly on each segment, not just at samples.
    /// Returns `None` if nothing was touched, otherwise the remaining pieces (possibly none).
    pub fn erase_circle(&self, c: [f64; 2], r: f64) -> Option<Vec<Vec<StrokePoint>>> {
        if !self.hit_circle(c, r) {
            return None;
        }
        let pts = &self.points;
        if pts.len() == 1 {
            return Some(Vec::new());
        }
        let lerp = |a: &StrokePoint, b: &StrokePoint, t: f64| StrokePoint {
            x: (a.x as f64 + (b.x - a.x) as f64 * t) as f32,
            y: (a.y as f64 + (b.y - a.y) as f64 * t) as f32,
            pressure: a.pressure + (b.pressure - a.pressure) * t as f32,
        };
        // Signed: < 0 inside the erased zone. Convex along a segment (distance to a point
        // minus a linear radius), so each segment has at most two crossings.
        let f = |p: &StrokePoint| ((c[0] - p.x as f64).powi(2) + (c[1] - p.y as f64).powi(2)).sqrt() - r - self.radius_at(p) as f64;
        let bisect = |a: &StrokePoint, b: &StrokePoint, mut lo: f64, mut hi: f64| {
            let inside_lo = f(&lerp(a, b, lo)) < 0.0;
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if (f(&lerp(a, b, mid)) < 0.0) == inside_lo {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            0.5 * (lo + hi)
        };
        let mut pieces: Vec<Vec<StrokePoint>> = Vec::new();
        let mut cur: Vec<StrokePoint> = Vec::new();
        if f(&pts[0]) >= 0.0 {
            cur.push(pts[0]);
        }
        for w in pts.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
            let len2 = dx * dx + dy * dy;
            let tc = if len2 > 0.0 { (((c[0] - a.x as f64) * dx + (c[1] - a.y as f64) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (fa, fc, fb) = (f(a) < 0.0, f(&lerp(a, b, tc)) < 0.0, f(b) < 0.0);
            // Crossings in order along the segment: (t, entering_erased_zone).
            let mut cross: Vec<(f64, bool)> = Vec::new();
            if fa != fc {
                cross.push((bisect(a, b, 0.0, tc), !fa));
            }
            if fc != fb {
                cross.push((bisect(a, b, tc, 1.0), !fc));
            }
            for (t, entering) in cross {
                let p = lerp(a, b, t);
                if entering {
                    cur.push(p);
                    pieces.push(std::mem::take(&mut cur));
                } else {
                    cur.push(p);
                }
            }
            if !fb {
                cur.push(*b);
            }
        }
        pieces.push(cur);
        pieces.retain(|p| p.len() >= 2);
        Some(pieces)
    }

    /// Does a circle of radius `r` at local point `c` touch the inked area?
    pub fn hit_circle(&self, c: [f64; 2], r: f64) -> bool {
        let bb = self.cache.bbox;
        if c[0] + r < bb[0] as f64 || c[0] - r > bb[2] as f64 || c[1] + r < bb[1] as f64 || c[1] - r > bb[3] as f64 {
            return false;
        }
        let pts = &self.points;
        if pts.len() == 1 {
            let p = pts[0];
            let d = ((c[0] - p.x as f64).powi(2) + (c[1] - p.y as f64).powi(2)).sqrt();
            return d <= r + self.radius_at(&p) as f64;
        }
        pts.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((c[0] - ax) * dx + (c[1] - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (ax + t * dx, ay + t * dy);
            let d = ((c[0] - px).powi(2) + (c[1] - py).powi(2)).sqrt();
            let rad = self.radius_at(&a) as f64 * (1.0 - t) + self.radius_at(&b) as f64 * t;
            d <= r + rad
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_and_hit() {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 4.0);
        s.push(StrokePoint { x: 0.0, y: 0.0, pressure: 1.0 });
        s.push(StrokePoint { x: 100.0, y: 0.0, pressure: 1.0 });
        assert_eq!(s.cache.bbox, [-2.0, -2.0, 102.0, 2.0]);
        assert!(s.hit_circle([50.0, 3.0], 1.5));
        assert!(!s.hit_circle([50.0, 10.0], 1.5));
        let mut t = s.clone();
        t.refresh_cache();
        assert_eq!(t.cache, s.cache);
    }

    fn line(n: usize) -> Stroke {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 4.0);
        for i in 0..=n {
            s.push(StrokePoint { x: i as f32 * 100.0 / n as f32, y: 0.0, pressure: 1.0 });
        }
        s
    }

    #[test]
    fn erase_middle_splits_in_two() {
        // Sparse points: the cut must be located inside the segments, not at samples.
        let s = line(2);
        let pieces = s.erase_circle([50.0, 1.0], 8.0).unwrap();
        assert_eq!(pieces.len(), 2);
        // Zone half-width along the line: sqrt((8+2)^2 - 1^2) ≈ 9.95.
        let cut = (10.0f32 * 10.0 - 1.0).sqrt();
        let end0 = pieces[0].last().unwrap().x;
        let start1 = pieces[1][0].x;
        assert!((end0 - (50.0 - cut)).abs() < 1e-3, "{end0}");
        assert!((start1 - (50.0 + cut)).abs() < 1e-3, "{start1}");
        assert_eq!(pieces[0][0].x, 0.0);
        assert_eq!(pieces[1].last().unwrap().x, 100.0);
    }

    #[test]
    fn erase_end_trims_and_miss_is_none() {
        let s = line(10);
        assert!(s.erase_circle([50.0, 30.0], 5.0).is_none());
        let pieces = s.erase_circle([100.0, 0.0], 10.0).unwrap();
        assert_eq!(pieces.len(), 1);
        assert!((pieces[0].last().unwrap().x - 88.0).abs() < 1e-3);
        let all = s.erase_circle([50.0, 0.0], 80.0).unwrap();
        assert!(all.is_empty());
    }
}
