//! The drawing: strokes plus a multi-resolution spatial hash per scale level.

use crate::camera::{Camera, LevelFrame};
use crate::image::{ImageId, RefImage};
use crate::stroke::{Stroke, StrokeId};
use num_bigint::BigInt;
use num_traits::One;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Cell sizes (log2, in level units) of the grids kept for every level.
const SHIFTS: [u32; 4] = [10, 14, 18, 22];
/// A stroke spanning more cells than this (per axis) at the finest grid goes to `oversized`.
const MAX_SPAN: u32 = 16;
/// Upper bound on cells enumerated per axis during a query.
const MAX_QUERY_SPAN: i64 = 24;
/// Strokes drawn this many levels deeper than the camera are < 1 px and skipped.
pub const LEVELS_BELOW: i32 = 16;
/// Strokes drawn this many levels shallower than the camera are astronomically larger
/// than the screen and skipped (their f32 geometry can't be positioned that precisely).
pub const LEVELS_ABOVE: i32 = 40;
/// Minimum on-screen size in pixels for a stroke to be drawn.
pub const MIN_PIXELS: f64 = 0.5;

type Cell = (BigInt, BigInt);

#[derive(Default, Debug)]
struct LevelIndex {
    grids: [HashMap<Cell, Vec<StrokeId>>; SHIFTS.len()],
    oversized: Vec<StrokeId>,
    count: usize,
}

/// Integer bbox of a stroke in level units: [min_x, min_y, max_x, max_y].
fn level_bbox(s: &Stroke) -> [BigInt; 4] {
    let b = s.cache.bbox;
    [
        &s.anchor[0] + BigInt::from(b[0].floor() as i64),
        &s.anchor[1] + BigInt::from(b[1].floor() as i64),
        &s.anchor[0] + BigInt::from(b[2].ceil() as i64),
        &s.anchor[1] + BigInt::from(b[3].ceil() as i64),
    ]
}

fn span(lo: &BigInt, hi: &BigInt) -> BigInt {
    hi - lo + BigInt::one()
}

impl LevelIndex {
    fn cells(bb: &[BigInt; 4], shift: u32) -> Option<Vec<Cell>> {
        let (x0, y0, x1, y1) = (&bb[0] >> shift, &bb[1] >> shift, &bb[2] >> shift, &bb[3] >> shift);
        if span(&x0, &x1) > BigInt::from(MAX_SPAN) || span(&y0, &y1) > BigInt::from(MAX_SPAN) {
            return None;
        }
        let mut out = Vec::new();
        let mut x = x0.clone();
        while x <= x1 {
            let mut y = y0.clone();
            while y <= y1 {
                out.push((x.clone(), y.clone()));
                y += 1;
            }
            x += 1;
        }
        Some(out)
    }

    fn insert(&mut self, s: &Stroke) {
        self.count += 1;
        let bb = level_bbox(s);
        if Self::cells(&bb, SHIFTS[0]).is_none() {
            self.oversized.push(s.id);
            return;
        }
        for (g, &shift) in self.grids.iter_mut().zip(SHIFTS.iter()) {
            for c in Self::cells(&bb, shift).unwrap_or_default() {
                g.entry(c).or_default().push(s.id);
            }
        }
    }

    fn remove(&mut self, s: &Stroke) {
        self.count -= 1;
        let bb = level_bbox(s);
        if Self::cells(&bb, SHIFTS[0]).is_none() {
            self.oversized.retain(|&i| i != s.id);
            return;
        }
        for (g, &shift) in self.grids.iter_mut().zip(SHIFTS.iter()) {
            for c in Self::cells(&bb, shift).unwrap_or_default() {
                if let Some(v) = g.get_mut(&c) {
                    v.retain(|&i| i != s.id);
                    if v.is_empty() {
                        g.remove(&c);
                    }
                }
            }
        }
    }
}

/// A stroke that should be drawn this frame.
#[derive(Clone, Debug)]
pub struct VisibleStroke {
    pub id: StrokeId,
    /// Draw order key (see [`Stroke::order`]).
    pub order: u64,
    pub level: i32,
    /// Screen position (px) of the stroke anchor, i.e. of local point (0, 0).
    pub offset: [f64; 2],
    /// Screen pixels per local unit (the view rotation is the camera's).
    pub scale: f64,
}

#[derive(Default, Debug)]
pub struct Document {
    strokes: BTreeMap<StrokeId, Stroke>,
    /// Reference photos, always drawn below the strokes (in id order).
    images: BTreeMap<ImageId, RefImage>,
    /// Saved views.
    pub bookmarks: Vec<crate::flight::Bookmark>,
    levels: BTreeMap<i32, LevelIndex>,
    next_id: StrokeId,
}

impl Document {
    pub fn new() -> Self {
        Self { next_id: 1, ..Default::default() }
    }

    pub fn from_strokes(strokes: Vec<Stroke>, next_id: StrokeId) -> Self {
        let mut d = Self::new();
        d.next_id = next_id.max(1);
        for mut s in strokes {
            s.refresh_cache();
            d.insert(s);
        }
        d
    }

    pub fn images(&self) -> impl Iterator<Item = &RefImage> {
        self.images.values()
    }

    pub fn image(&self, id: ImageId) -> Option<&RefImage> {
        self.images.get(&id)
    }

    pub fn image_mut(&mut self, id: ImageId) -> Option<&mut RefImage> {
        self.images.get_mut(&id)
    }

    pub fn insert_image(&mut self, img: RefImage) {
        self.next_id = self.next_id.max(img.id + 1);
        self.images.insert(img.id, img);
    }

    pub fn remove_image(&mut self, id: ImageId) -> Option<RefImage> {
        self.images.remove(&id)
    }

    pub fn alloc_id(&mut self) -> StrokeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn next_id(&self) -> StrokeId {
        self.next_id
    }

    pub fn len(&self) -> usize {
        self.strokes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    pub fn get(&self, id: StrokeId) -> Option<&Stroke> {
        self.strokes.get(&id)
    }

    pub fn strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.strokes.values()
    }

    pub fn insert(&mut self, s: Stroke) {
        if s.points.is_empty() {
            return;
        }
        if let Some(old) = self.strokes.remove(&s.id) {
            self.unindex(&old);
        }
        self.next_id = self.next_id.max(s.id + 1);
        self.levels.entry(s.level).or_default().insert(&s);
        self.strokes.insert(s.id, s);
    }

    pub fn remove(&mut self, id: StrokeId) -> Option<Stroke> {
        let s = self.strokes.remove(&id)?;
        self.unindex(&s);
        Some(s)
    }

    fn unindex(&mut self, s: &Stroke) {
        if let Some(l) = self.levels.get_mut(&s.level) {
            l.remove(s);
            if l.count == 0 {
                self.levels.remove(&s.level);
            }
        }
    }

    /// Strokes intersecting the screen rectangle `rect` = [x0, y0, x1, y1] (px), in draw order.
    pub fn query(&self, cam: &Camera, viewport: [f64; 2], rect: [f64; 4]) -> Vec<VisibleStroke> {
        let z = cam.level();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (&level, idx) in self.levels.range(z - LEVELS_ABOVE..=z + LEVELS_BELOW) {
            let f = cam.level_frame(level, viewport);
            let mut candidates: Vec<StrokeId> = idx.oversized.clone();
            // Query rectangle in level units relative to the reference point: bounding box of
            // the (possibly rotated) screen rectangle.
            let mut lo = [f64::MAX; 2];
            let mut hi = [f64::MIN; 2];
            for (sx, sy) in [(rect[0], rect[1]), (rect[2], rect[1]), (rect[0], rect[3]), (rect[2], rect[3])] {
                let v = f.screen_vec_to_local([sx - f.half_viewport[0], sy - f.half_viewport[1]]);
                for a in 0..2 {
                    lo[a] = lo[a].min(v[a] + f.frac[a]);
                    hi[a] = hi[a].max(v[a] + f.frac[a]);
                }
            }
            let lo = [lo[0].floor() - 1.0, lo[1].floor() - 1.0];
            let hi = [hi[0].ceil() + 1.0, hi[1].ceil() + 1.0];
            let ext = (hi[0] - lo[0]).max(hi[1] - lo[1]);
            let gi = SHIFTS
                .iter()
                .position(|&s| ext / (1u64 << s) as f64 + 2.0 <= MAX_QUERY_SPAN as f64)
                .unwrap_or(SHIFTS.len() - 1);
            let shift = SHIFTS[gi];
            let grid = &idx.grids[gi];
            if (grid.len() as f64) < (ext / (1u64 << shift) as f64 + 2.0).powi(2) || ext > 1e15 {
                // Fewer occupied cells than cells in view: scan the occupied ones.
                let bx0 = (&f.reference[0] + BigInt::from(lo[0].max(-1e15) as i64)) >> shift;
                let by0 = (&f.reference[1] + BigInt::from(lo[1].max(-1e15) as i64)) >> shift;
                let bx1 = (&f.reference[0] + BigInt::from(hi[0].min(1e15) as i64)) >> shift;
                let by1 = (&f.reference[1] + BigInt::from(hi[1].min(1e15) as i64)) >> shift;
                for ((cx, cy), ids) in grid {
                    if (ext > 1e15) || (*cx >= bx0 && *cx <= bx1 && *cy >= by0 && *cy <= by1) {
                        candidates.extend_from_slice(ids);
                    }
                }
            } else {
                let x0 = (&f.reference[0] + BigInt::from(lo[0] as i64)) >> shift;
                let y0 = (&f.reference[1] + BigInt::from(lo[1] as i64)) >> shift;
                let x1 = (&f.reference[0] + BigInt::from(hi[0] as i64)) >> shift;
                let y1 = (&f.reference[1] + BigInt::from(hi[1] as i64)) >> shift;
                let mut x = x0;
                while x <= x1 {
                    let mut y = y0.clone();
                    while y <= y1 {
                        if let Some(ids) = grid.get(&(x.clone(), y.clone())) {
                            candidates.extend_from_slice(ids);
                        }
                        y += 1;
                    }
                    x += 1;
                }
            }
            for id in candidates {
                if !seen.insert(id) {
                    continue;
                }
                let s = &self.strokes[&id];
                if let Some(v) = self.visible(s, &f, rect) {
                    out.push(v);
                }
            }
        }
        out.sort_unstable_by_key(|v| (v.order, v.id));
        out
    }

    fn visible(&self, s: &Stroke, f: &LevelFrame, rect: [f64; 4]) -> Option<VisibleStroke> {
        let o = f.anchor_to_screen(&s.anchor, s.cache.anchor_fast);
        let b = s.cache.bbox;
        // Screen bounding box of the (rotated) local bounding box.
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (bx, by) in [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])] {
            let v = f.local_vec_to_screen([bx as f64, by as f64]);
            x0 = x0.min(o[0] + v[0]);
            y0 = y0.min(o[1] + v[1]);
            x1 = x1.max(o[0] + v[0]);
            y1 = y1.max(o[1] + v[1]);
        }
        if x1 < rect[0] || x0 > rect[2] || y1 < rect[1] || y0 > rect[3] {
            return None;
        }
        if (x1 - x0).max(y1 - y0) < MIN_PIXELS {
            return None;
        }
        Some(VisibleStroke { id: s.id, order: s.order, level: s.level, offset: o, scale: f.scale })
    }

    /// Ids of strokes touched by a circle of `radius` px around screen point `p`.
    pub fn hit_test(&self, cam: &Camera, viewport: [f64; 2], p: [f64; 2], radius: f64) -> Vec<StrokeId> {
        let rect = [p[0] - radius, p[1] - radius, p[0] + radius, p[1] + radius];
        let rot = cam.rot();
        self.query(cam, viewport, rect)
            .into_iter()
            .filter(|v| {
                let s = &self.strokes[&v.id];
                let local = crate::camera::unrotate([(p[0] - v.offset[0]) / v.scale, (p[1] - v.offset[1]) / v.scale], rot);
                s.hit_circle(local, radius / v.scale)
            })
            .map(|v| v.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::StrokePoint;

    const VP: [f64; 2] = [1600.0, 1000.0];
    const FULL: [f64; 4] = [0.0, 0.0, 1600.0, 1000.0];

    /// Draws a short horizontal stroke through screen point `p` at the camera's current zoom.
    fn draw(doc: &mut Document, cam: &Camera, p: [f64; 2]) -> StrokeId {
        let level = cam.level();
        let w = cam.screen_to_world(p, VP).mul_pow2(level);
        let anchor = [w.x.floor_at(0), w.y.floor_at(0)];
        let f = cam.level_frame(level, VP);
        let id = doc.alloc_id();
        let mut s = Stroke::new(id, level, anchor.clone(), [0, 0, 0, 255], 4.0);
        for dx in [-20.0, 0.0, 20.0] {
            let l = f.screen_to_local([p[0] + dx, p[1]], &anchor, None);
            s.push(StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: 1.0 });
        }
        doc.insert(s);
        id
    }

    #[test]
    fn query_finds_and_culls() {
        let mut doc = Document::new();
        let cam = Camera::default();
        let a = draw(&mut doc, &cam, [100.0, 100.0]);
        let mut far = cam.clone();
        far.pan_px([-100_000.0, 0.0]);
        let b = draw(&mut doc, &far, [800.0, 500.0]);
        let v: Vec<_> = doc.query(&cam, VP, FULL).iter().map(|v| v.id).collect();
        assert_eq!(v, vec![a]);
        let v: Vec<_> = doc.query(&far, VP, FULL).iter().map(|v| v.id).collect();
        assert_eq!(v, vec![b]);
    }

    #[test]
    fn deep_stroke_stays_put_and_hides_when_tiny() {
        let mut doc = Document::new();
        let mut cam = Camera::default();
        let p = [400.0, 300.0];
        // Dive 2^300 (~10^90) at p, draw, then check pixel position stability.
        for _ in 0..300 {
            cam.zoom_at(p, VP, 1.0);
        }
        let id = draw(&mut doc, &cam, p);
        let v = doc.query(&cam, VP, FULL);
        assert_eq!(v.len(), 1);
        let s = doc.get(id).unwrap();
        let first = s.points[1];
        let sx = v[0].offset[0] + first.x as f64 * v[0].scale;
        let sy = v[0].offset[1] + first.y as f64 * v[0].scale;
        assert!((sx - p[0]).abs() < 0.01 && (sy - p[1]).abs() < 0.01, "{sx},{sy}");
        // Tiny pans keep sub-pixel accuracy (no jitter).
        let mut c2 = cam.clone();
        c2.pan_px([0.25, 0.0]);
        let v2 = doc.query(&c2, VP, FULL);
        assert!((v2[0].offset[0] - v[0].offset[0] - 0.25).abs() < 1e-6);
        // Zoom back out: the stroke becomes sub-pixel and is culled.
        let mut out = cam.clone();
        for _ in 0..300 {
            out.zoom_at(p, VP, -1.0);
        }
        assert!(doc.query(&out, VP, FULL).is_empty());
        // ...but a stroke at the origin level is visible again.
        let base = draw(&mut doc, &out, [10.0, 10.0]);
        assert_eq!(doc.query(&out, VP, FULL).iter().map(|v| v.id).collect::<Vec<_>>(), vec![base]);
    }

    #[test]
    fn zoom_into_old_stroke() {
        let mut doc = Document::new();
        let mut cam = Camera::default();
        let id = draw(&mut doc, &cam, [500.0, 500.0]);
        for _ in 0..30 {
            cam.zoom_at([500.0, 500.0], VP, 1.0);
        }
        let v = doc.query(&cam, VP, FULL);
        assert_eq!(v.iter().map(|v| v.id).collect::<Vec<_>>(), vec![id]);
        assert!(!doc.hit_test(&cam, VP, [500.0, 500.0], 5.0).is_empty());
    }

    #[test]
    fn hit_and_remove() {
        let mut doc = Document::new();
        let cam = Camera::default();
        let id = draw(&mut doc, &cam, [300.0, 300.0]);
        assert_eq!(doc.hit_test(&cam, VP, [310.0, 301.0], 3.0), vec![id]);
        assert!(doc.hit_test(&cam, VP, [310.0, 330.0], 3.0).is_empty());
        doc.remove(id);
        assert!(doc.query(&cam, VP, FULL).is_empty());
        assert!(doc.levels.is_empty());
    }

    #[test]
    fn query_10k_strokes_is_fast() {
        let mut doc = Document::new();
        let mut cam = Camera::default();
        for i in 0..10_000 {
            if i % 1000 == 0 {
                cam.zoom_at([800.0, 500.0], VP, 2.0);
            }
            let p = [(i % 100) as f64 * 16.0, ((i / 100) % 100) as f64 * 10.0];
            draw(&mut doc, &cam, p);
        }
        let t = std::time::Instant::now();
        let n = doc.query(&cam, VP, FULL).len();
        let dt = t.elapsed();
        assert!(n >= 1000, "{n}");
        // Generous bound for debug builds on slow CI; release is far faster.
        assert!(dt.as_millis() < 200, "{dt:?}");
    }

    #[test]
    fn rotated_view_query_and_hit() {
        let mut doc = Document::new();
        let mut cam = Camera::default();
        cam.zoom_at([0.0, 0.0], VP, 120.0);
        let id = draw(&mut doc, &cam, [1500.0, 900.0]);
        // Rotate 90° around the screen centre: the stroke moves to a different screen spot,
        // must still be found there and nowhere else.
        cam.rotate_at([800.0, 500.0], VP, std::f64::consts::FRAC_PI_2);
        let s = doc.get(id).unwrap();
        let f = cam.level_frame(s.level, VP);
        let o = f.anchor_to_screen(&s.anchor, s.cache.anchor_fast);
        let mid = s.points[1];
        let v = f.local_vec_to_screen([mid.x as f64, mid.y as f64]);
        let p = [o[0] + v[0], o[1] + v[1]];
        // (1500, 900) relative to the centre (700, 400) rotated by 90° → (-400, 700).
        assert!((p[0] - 400.0).abs() < 1e-3 && (p[1] - 1200.0).abs() < 1e-3, "{p:?}");
        assert_eq!(doc.hit_test(&cam, VP, p, 3.0), vec![id]);
        assert!(doc.hit_test(&cam, VP, [1500.0, 900.0], 3.0).is_empty());
        assert_eq!(doc.query(&cam, VP, [p[0] - 5.0, p[1] - 5.0, p[0] + 5.0, p[1] + 5.0]).len(), 1);
        assert!(doc.query(&cam, VP, [1400.0, 850.0, 1600.0, 950.0]).is_empty());
    }
}
