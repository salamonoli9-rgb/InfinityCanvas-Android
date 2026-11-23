//! Lasso selection: which strokes a lasso encloses, their bounding box on screen, and moving /
//! scaling them (previewed through the per-stroke render transform, committed exactly).

use canvas_core::camera::Camera;
use canvas_core::document::{Document, VisibleStroke};
use canvas_core::stroke::{Stroke, StrokeId};
use num_bigint::BigInt;

/// A transform of the selection, in screen space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Xform {
    Move([f64; 2]),
    /// Uniform scale by `k` around a screen point.
    Scale { center: [f64; 2], k: f64 },
}

impl Xform {
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        match *self {
            Xform::Move(d) => [p[0] + d[0], p[1] + d[1]],
            Xform::Scale { center: c, k } => [c[0] + (p[0] - c[0]) * k, c[1] + (p[1] - c[1]) * k],
        }
    }

    /// Preview: the same transform applied to a stroke's render placement.
    pub fn apply_visible(&self, v: &mut VisibleStroke) {
        v.offset = self.apply(v.offset);
        if let Xform::Scale { k, .. } = *self {
            v.scale *= k;
        }
    }
}

/// Even-odd point-in-polygon.
pub fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// Screen positions of (up to ~32 of) a stroke's points.
fn screen_points(s: &Stroke, cam: &Camera, vp: [f64; 2]) -> Vec<[f64; 2]> {
    let f = cam.level_frame(s.level, vp);
    let o = f.anchor_to_screen(&s.anchor, s.cache.anchor_fast);
    let step = (s.points.len() / 32).max(1);
    s.points
        .iter()
        .step_by(step)
        .filter(|p| p.pressure >= 0.0)
        .map(|p| {
            let v = f.local_vec_to_screen([p.x as f64, p.y as f64]);
            [o[0] + v[0], o[1] + v[1]]
        })
        .collect()
}

/// Strokes (not erasers) with at least half of their points inside the lasso.
pub fn lasso(doc: &Document, cam: &Camera, vp: [f64; 2], poly: &[[f64; 2]]) -> Vec<StrokeId> {
    if poly.len() < 3 {
        return Vec::new();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in poly {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    doc.query(cam, vp, [x0, y0, x1, y1])
        .iter()
        .filter_map(|v| {
            let s = doc.get(v.id)?;
            if s.is_eraser() {
                return None;
            }
            let pts = screen_points(s, cam, vp);
            let n_in = pts.iter().filter(|p| inside(poly, **p)).count();
            (!pts.is_empty() && n_in * 2 >= pts.len()).then_some(v.id)
        })
        .collect()
}

/// Screen bounding box [x0, y0, x1, y1] of the strokes (ink extent), with a transform preview.
pub fn bbox(doc: &Document, cam: &Camera, vp: [f64; 2], ids: &[StrokeId], x: Option<Xform>) -> Option<[f64; 4]> {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for id in ids {
        let Some(s) = doc.get(*id) else { continue };
        let f = cam.level_frame(s.level, vp);
        let o = f.anchor_to_screen(&s.anchor, s.cache.anchor_fast);
        let c = s.cache.bbox;
        for (bx, by) in [(c[0], c[1]), (c[2], c[1]), (c[0], c[3]), (c[2], c[3])] {
            let v = f.local_vec_to_screen([bx as f64, by as f64]);
            let p = [o[0] + v[0], o[1] + v[1]];
            let p = x.map_or(p, |x| x.apply(p));
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    }
    (b[0] <= b[2]).then_some(b)
}

/// A copy of `s` with id `id`, transformed by `x` (screen space) exactly: big moves go into
/// the integer anchor, only the fraction into the f32 points.
pub fn transformed(s: &Stroke, cam: &Camera, vp: [f64; 2], x: Xform, id: StrokeId) -> Stroke {
    let f = cam.level_frame(s.level, vp);
    let mut t = s.clone();
    t.id = id;
    match x {
        Xform::Move(d) => {
            let dl = f.screen_vec_to_local(d);
            let int = [dl[0].floor(), dl[1].floor()];
            t.anchor = [&s.anchor[0] + BigInt::from(int[0] as i64), &s.anchor[1] + BigInt::from(int[1] as i64)];
            let frac = [(dl[0] - int[0]) as f32, (dl[1] - int[1]) as f32];
            for p in &mut t.points {
                p.x += frac[0];
                p.y += frac[1];
            }
        }
        Xform::Scale { center, k } => {
            let pc = f.screen_to_local(center, &s.anchor, s.cache.anchor_fast);
            for p in &mut t.points {
                p.x = (pc[0] + (p.x as f64 - pc[0]) * k) as f32;
                p.y = (pc[1] + (p.y as f64 - pc[1]) * k) as f32;
            }
            t.width = (t.width as f64 * k) as f32;
        }
    }
    t.refresh_cache();
    t
}

/// What a press on the selection does.
pub enum Grab {
    /// Inside the box: move.
    Move,
    /// On the corner handle: scale around the opposite corner.
    Scale { center: [f64; 2] },
}

/// Decides what pressing at `p` does with a selection whose screen box is `b`.
pub fn grab(b: [f64; 4], p: [f64; 2], handle: f64) -> Option<Grab> {
    if (p[0] - b[2]).hypot(p[1] - b[3]) <= handle {
        return Some(Grab::Scale { center: [b[0], b[1]] });
    }
    let m = handle * 0.5;
    (p[0] >= b[0] - m && p[0] <= b[2] + m && p[1] >= b[1] - m && p[1] <= b[3] + m).then_some(Grab::Move)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::stroke::StrokePoint;

    #[test]
    fn exact_move_and_scale() {
        let mut cam = Camera::default();
        cam.zoom_at([0.0, 0.0], [800.0, 600.0], 200.0);
        let vp = [800.0, 600.0];
        let level = cam.level();
        let w = cam.screen_to_world([100.0, 100.0], vp).mul_pow2(level);
        let mut s = Stroke::new(1, level, [w.x.floor_at(0), w.y.floor_at(0)], [0, 0, 0, 255], 2.0);
        let f = cam.level_frame(level, vp);
        for x in [100.0, 200.0] {
            let l = f.screen_to_local([x, 100.0], &s.anchor, None);
            s.push(StrokePoint { x: l[0] as f32, y: l[1] as f32, pressure: 1.0 });
        }
        s.refresh_cache();
        let screen = |s: &Stroke| screen_points(s, &cam, vp);
        let moved = transformed(&s, &cam, vp, Xform::Move([1e6, -3.5]), 2);
        let p = screen(&moved);
        assert!((p[0][0] - (100.0 + 1e6)).abs() < 1e-3 && (p[0][1] - 96.5).abs() < 1e-3, "{:?}", p[0]);
        assert!(moved.points[0].x.abs() < 2.0, "a big move goes into the anchor");
        let scaled = transformed(&s, &cam, vp, Xform::Scale { center: [100.0, 100.0], k: 3.0 }, 3);
        let p = screen(&scaled);
        assert!((p[1][0] - 400.0).abs() < 1e-3 && (scaled.width - 6.0).abs() < 1e-5);
    }

    #[test]
    fn lasso_and_grab() {
        let sq = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        assert!(inside(&sq, [50.0, 50.0]) && !inside(&sq, [150.0, 50.0]));
        let b = [10.0, 10.0, 110.0, 60.0];
        assert!(matches!(grab(b, [108.0, 58.0], 20.0), Some(Grab::Scale { center }) if center == [10.0, 10.0]));
        assert!(matches!(grab(b, [50.0, 30.0], 20.0), Some(Grab::Move)));
        assert!(grab(b, [300.0, 300.0], 20.0).is_none());
    }
}
