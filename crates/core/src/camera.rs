//! Camera with exact position and logarithmic zoom.
//!
//! Screen pixels per world unit = `2^log_zoom`. Screen space has its origin in the top-left
//! corner and the y axis pointing down, world space uses the same orientation.

use crate::coords::{Dyadic, WorldPos, ldexp};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};

/// How many bits below one screen pixel the camera position keeps.
const EXTRA_BITS: i32 = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub center: WorldPos,
    pub log_zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self { center: WorldPos::origin(), log_zoom: 0.0 }
    }
}

/// Integer part of the zoom exponent; also the "scale level" new strokes are drawn at.
pub fn level_for(log_zoom: f64) -> i32 {
    log_zoom.floor() as i32
}

impl Camera {
    pub fn level(&self) -> i32 {
        level_for(self.log_zoom)
    }

    fn split_zoom(&self) -> (i32, f64) {
        let k = self.level();
        (k, self.log_zoom - k as f64)
    }

    /// World-space vector that spans `d` screen pixels.
    pub fn screen_delta_to_world(&self, d: [f64; 2]) -> WorldPos {
        let (k, f) = self.split_zoom();
        let s = (-f).exp2();
        WorldPos::new(
            Dyadic::from_f64(d[0] * s).mul_pow2(-k),
            Dyadic::from_f64(d[1] * s).mul_pow2(-k),
        )
    }

    pub fn screen_to_world(&self, p: [f64; 2], viewport: [f64; 2]) -> WorldPos {
        let d = [p[0] - viewport[0] * 0.5, p[1] - viewport[1] * 0.5];
        self.center.add(&self.screen_delta_to_world(d))
    }

    pub fn world_to_screen(&self, w: &WorldPos, viewport: [f64; 2]) -> [f64; 2] {
        let (k, f) = self.split_zoom();
        let s = f.exp2();
        let d = w.sub(&self.center).mul_pow2(k).to_f64();
        [d[0] * s + viewport[0] * 0.5, d[1] * s + viewport[1] * 0.5]
    }

    fn trim(&mut self) {
        let e = -(self.level() + EXTRA_BITS);
        self.center.truncate(e);
    }

    /// Moves the content by `d` screen pixels.
    pub fn pan_px(&mut self, d: [f64; 2]) {
        self.center = self.center.sub(&self.screen_delta_to_world(d));
        self.trim();
    }

    /// Zooms by `dlog` binary orders of magnitude keeping screen point `p` fixed.
    pub fn zoom_at(&mut self, p: [f64; 2], viewport: [f64; 2], dlog: f64) {
        if !dlog.is_finite() || dlog == 0.0 {
            return;
        }
        let anchor = self.screen_to_world(p, viewport);
        self.log_zoom += dlog;
        let d = [p[0] - viewport[0] * 0.5, p[1] - viewport[1] * 0.5];
        self.center = anchor.sub(&self.screen_delta_to_world(d));
        self.trim();
    }

    /// Per-frame helper for converting stroke-local coordinates at `level` to the screen.
    pub fn level_frame(&self, level: i32, viewport: [f64; 2]) -> LevelFrame {
        let c = self.center.mul_pow2(level);
        let rx = c.x.floor_at(0);
        let ry = c.y.floor_at(0);
        let fx = c.x.sub(&Dyadic::new(rx.clone(), 0)).to_f64();
        let fy = c.y.sub(&Dyadic::new(ry.clone(), 0)).to_f64();
        let fast = rx.to_i128().zip(ry.to_i128()).map(|(a, b)| [a, b]);
        LevelFrame {
            level,
            reference: [rx, ry],
            reference_fast: fast,
            frac: [fx, fy],
            scale: (self.log_zoom - level as f64).exp2(),
            half_viewport: [viewport[0] * 0.5, viewport[1] * 0.5],
        }
    }
}

/// Camera expressed in the integer grid of one scale level.
///
/// A stroke at `level` stores an integer `anchor` (in units of `2^-level` world units) and
/// local f32 points relative to it. Its screen position is
/// `((anchor - reference) - frac + local) * scale + half_viewport`, where only the first
/// difference involves big integers and it is small for anything near the screen.
#[derive(Clone, Debug)]
pub struct LevelFrame {
    pub level: i32,
    pub reference: [BigInt; 2],
    reference_fast: Option<[i128; 2]>,
    pub frac: [f64; 2],
    /// Screen pixels per level unit.
    pub scale: f64,
    pub half_viewport: [f64; 2],
}

impl LevelFrame {
    /// `anchor - camera` in level units as f64 (exact difference, then rounded).
    pub fn anchor_offset(&self, anchor: &[BigInt; 2], anchor_fast: Option<[i128; 2]>) -> [f64; 2] {
        let (dx, dy) = match (anchor_fast, self.reference_fast) {
            (Some(a), Some(r)) => match (a[0].checked_sub(r[0]), a[1].checked_sub(r[1])) {
                (Some(x), Some(y)) => (x as f64, y as f64),
                _ => self.slow_diff(anchor),
            },
            _ => self.slow_diff(anchor),
        };
        [dx - self.frac[0], dy - self.frac[1]]
    }

    fn slow_diff(&self, anchor: &[BigInt; 2]) -> (f64, f64) {
        let dx = &anchor[0] - &self.reference[0];
        let dy = &anchor[1] - &self.reference[1];
        (big_to_f64(&dx), big_to_f64(&dy))
    }

    /// Screen position of the anchor (top-left origin, pixels).
    pub fn anchor_to_screen(&self, anchor: &[BigInt; 2], fast: Option<[i128; 2]>) -> [f64; 2] {
        let o = self.anchor_offset(anchor, fast);
        [o[0] * self.scale + self.half_viewport[0], o[1] * self.scale + self.half_viewport[1]]
    }

    /// Converts a screen point into level-local units relative to `anchor`.
    pub fn screen_to_local(&self, p: [f64; 2], anchor: &[BigInt; 2], fast: Option<[i128; 2]>) -> [f64; 2] {
        let o = self.anchor_offset(anchor, fast);
        [
            (p[0] - self.half_viewport[0]) / self.scale - o[0],
            (p[1] - self.half_viewport[1]) / self.scale - o[1],
        ]
    }
}

pub fn big_to_f64(v: &BigInt) -> f64 {
    Dyadic::new(v.clone(), 0).to_f64()
}

/// `2^e` as f64 for any exponent (saturating).
pub fn exp2i(e: i64) -> f64 {
    ldexp(1.0, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f64; 2] = [1920.0, 1080.0];

    fn close(a: [f64; 2], b: [f64; 2], eps: f64) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps
    }

    #[test]
    fn screen_world_roundtrip_at_extreme_depths() {
        for depth in [0.0, 90.0, 300.0, -90.0, -300.0, 1000.5] {
            let mut cam = Camera::default();
            cam.zoom_at([700.0, 300.0], VP, depth);
            for p in [[0.0, 0.0], [1919.5, 1079.25], [960.123, 540.987]] {
                let w = cam.screen_to_world(p, VP);
                let back = cam.world_to_screen(&w, VP);
                assert!(close(p, back, 1e-6), "depth {depth}: {p:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed() {
        let mut cam = Camera::default();
        let p = [123.0, 456.0];
        let w0 = cam.screen_to_world(p, VP);
        for _ in 0..300 {
            cam.zoom_at(p, VP, 0.37); // ~10^33 in total
        }
        let back = cam.world_to_screen(&w0, VP);
        assert!(close(p, back, 1e-6), "{back:?}");
    }

    #[test]
    fn dive_to_2_pow_minus_90_and_back() {
        // Zoom in 2^300 (~10^90) at an off-centre point, pan around, come back out.
        let mut cam = Camera::default();
        let start = cam.clone();
        let p = [1500.0, 200.0];
        for _ in 0..3000 {
            cam.zoom_at(p, VP, 0.1);
        }
        assert!((cam.log_zoom - 300.0).abs() < 1e-9);
        cam.pan_px([10.0, -4.0]);
        cam.pan_px([-10.0, 4.0]);
        for _ in 0..3000 {
            cam.zoom_at(p, VP, -0.1);
        }
        let a = cam.world_to_screen(&WorldPos::origin(), VP);
        let b = start.world_to_screen(&WorldPos::origin(), VP);
        assert!(close(a, b, 1e-3), "{a:?} vs {b:?}");
    }

    #[test]
    fn level_frame_matches_world_to_screen() {
        let mut cam = Camera::default();
        cam.zoom_at([10.0, 10.0], VP, 250.3);
        cam.pan_px([3.3, 7.7]);
        let level = cam.level() - 2;
        let f = cam.level_frame(level, VP);
        let w = cam.screen_to_world([800.0, 100.0], VP);
        let ax = w.x.mul_pow2(level).floor_at(0);
        let ay = w.y.mul_pow2(level).floor_at(0);
        let anchor = [ax.clone(), ay.clone()];
        let s1 = f.anchor_to_screen(&anchor, None);
        let s2 = cam.world_to_screen(
            &WorldPos::new(Dyadic::new(ax, -level), Dyadic::new(ay, -level)),
            VP,
        );
        assert!(close(s1, s2, 1e-6), "{s1:?} vs {s2:?}");
    }
}
