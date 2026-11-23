//! A canvas grid that adapts to the zoom (cells always 24–48 points on screen, every 4th line
//! stronger), exact at any depth, and snapping of screen points to its crossings.

use canvas_core::camera::{Camera, rotate, unrotate};
use num_bigint::BigInt;
use num_traits::ToPrimitive;

pub struct Grid {
    /// Screen px between lines.
    pub step: f64,
    /// Screen position of the crossing nearest to... the reference crossing (an exact grid
    /// point near the screen centre).
    origin: [f64; 2],
    rot: [f64; 2],
    /// Index (mod 4) of the reference crossing's lines, for the stronger lines.
    major: [i64; 2],
}

/// Cells on screen are at least this big (logical points).
const MIN_CELL: f64 = 24.0;

impl Grid {
    pub fn new(cam: &Camera, viewport: [f64; 2], ppp: f64) -> Self {
        // Cell size in world units: 2^e with 2^(e + log_zoom) ≥ MIN_CELL·ppp.
        let e = ((MIN_CELL * ppp).log2() - cam.log_zoom).ceil() as i32;
        let step = (e as f64 + cam.log_zoom).exp2();
        // The camera centre in cell units, exactly: integer part (for the major lines) and
        // fraction (for the position).
        let cx = cam.center.x.mul_pow2(-e);
        let cy = cam.center.y.mul_pow2(-e);
        let (ix, iy) = (cx.floor_at(0), cy.floor_at(0));
        let (fx, fy) = (cx.fract_f64(), cy.fract_f64());
        let m4 = |v: &BigInt| (v % BigInt::from(4)).to_i64().unwrap_or(0).rem_euclid(4);
        let rot = cam.rot();
        let d = rotate([-fx * step, -fy * step], rot);
        Self { step, origin: [viewport[0] * 0.5 + d[0], viewport[1] * 0.5 + d[1]], rot, major: [m4(&ix), m4(&iy)] }
    }

    /// Lines covering the screen: (from, to, strong).
    pub fn lines(&self, viewport: [f64; 2]) -> Vec<([f64; 2], [f64; 2], bool)> {
        let reach = viewport[0].hypot(viewport[1]);
        let n = (reach / self.step).ceil() as i64 + 1;
        let mut out = Vec::new();
        for axis in 0..2 {
            for k in -n..=n {
                // A line through the crossing k cells away along `axis`, running along the other.
                let (off, dir) = if axis == 0 { ([k as f64 * self.step, 0.0], [0.0, 1.0]) } else { ([0.0, k as f64 * self.step], [1.0, 0.0]) };
                let o = rotate(off, self.rot);
                let d = rotate(dir, self.rot);
                let c = [self.origin[0] + o[0], self.origin[1] + o[1]];
                let strong = (self.major[axis] + k).rem_euclid(4) == 0;
                out.push(([c[0] - d[0] * reach, c[1] - d[1] * reach], [c[0] + d[0] * reach, c[1] + d[1] * reach], strong));
            }
        }
        out
    }

    /// The grid crossing nearest to screen point `p`.
    pub fn snap(&self, p: [f64; 2]) -> [f64; 2] {
        let v = unrotate([p[0] - self.origin[0], p[1] - self.origin[1]], self.rot);
        let q = [(v[0] / self.step).round() * self.step, (v[1] / self.step).round() * self.step];
        let r = rotate(q, self.rot);
        [self.origin[0] + r[0], self.origin[1] + r[1]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f64; 2] = [800.0, 600.0];

    #[test]
    fn cell_size_adapts_and_snaps_to_world_points() {
        for depth in [0.0, 37.3, 300.0, -50.0] {
            let mut cam = Camera::default();
            cam.zoom_at([123.0, 456.0], VP, depth);
            cam.pan_px([3.7, -11.2]);
            let g = Grid::new(&cam, VP, 1.0);
            assert!(g.step >= 24.0 && g.step < 48.0, "{}", g.step);
            // A snapped point is a crossing: snapping it again doesn't move it, and moving the
            // camera by whole cells keeps it a crossing.
            let p = g.snap([401.3, 299.9]);
            let q = g.snap(p);
            assert!((p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6);
            let w = cam.screen_to_world(p, VP);
            let mut cam2 = cam.clone();
            cam2.pan_px([2.0 * g.step, -3.0 * g.step]);
            let g2 = Grid::new(&cam2, VP, 1.0);
            let p2 = cam2.world_to_screen(&w, VP);
            let s2 = g2.snap(p2);
            assert!((s2[0] - p2[0]).abs() < 1e-6 && (s2[1] - p2[1]).abs() < 1e-6, "depth {depth}: {p2:?} vs {s2:?}");
        }
    }

    #[test]
    fn every_fourth_line_is_strong() {
        let g = Grid::new(&Camera::default(), VP, 1.0);
        let lines = g.lines(VP);
        let strong = lines.iter().filter(|l| l.2).count();
        assert!(strong * 4 >= lines.len() - 8 && strong * 4 <= lines.len() + 8, "{strong} of {}", lines.len());
    }
}
