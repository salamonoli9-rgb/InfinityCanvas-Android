//! Smooth camera flight between two views (bookmarks): zoom out, travel, zoom in.
//!
//! The position is interpolated exactly (`start + delta · f` with dyadic arithmetic), so a
//! flight between places 10^100 apart arrives precisely.

use crate::camera::{Camera, wrap_angle};
use crate::coords::{Dyadic, WorldPos};
use serde::{Deserialize, Serialize};

/// A named view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub camera: Camera,
}

#[derive(Clone, Debug)]
pub struct Flight {
    from: Camera,
    delta: WorldPos,
    z0: f64,
    z1: f64,
    r0: f64,
    dr: f64,
    /// How far (log2) the camera backs out at mid-flight beyond the straight zoom interpolation.
    bump: f64,
    /// Seconds.
    pub duration: f64,
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

impl Flight {
    pub fn new(from: &Camera, to: &Camera, viewport: [f64; 2]) -> Self {
        let delta = to.center.sub(&from.center);
        let (z0, z1) = (from.log_zoom, to.log_zoom);
        // Distance between the two centres, log2 of world units.
        let dist = delta.x.log2_abs().max(delta.y.log2_abs());
        // Zoom at which that distance fits comfortably on screen.
        let z_fit = (0.6 * viewport[0].min(viewport[1])).log2() - dist;
        let mid = 0.5 * (z0 + z1);
        let peak = if dist.is_finite() { mid.min(z_fit) } else { mid };
        let bump = mid - peak;
        let travel = (z0 - peak).abs() + (z1 - peak).abs();
        let duration = (0.7 + 0.07 * travel).clamp(0.7, 4.0);
        Self { from: from.clone(), delta, z0, z1, r0: from.rotation, dr: wrap_angle(to.rotation - from.rotation), bump, duration }
    }

    /// Camera at time `t` seconds (clamped to the flight).
    pub fn at(&self, t: f64) -> Camera {
        let u = (t / self.duration).clamp(0.0, 1.0);
        let e = smooth(u);
        let mut cam = self.from.clone();
        cam.log_zoom = self.z0 + (self.z1 - self.z0) * e - self.bump * (std::f64::consts::PI * u).sin();
        let f = Dyadic::from_f64(e);
        cam.center = self.from.center.add(&WorldPos::new(self.delta.x.mul(&f), self.delta.y.mul(&f)));
        cam.rotation = wrap_angle(self.r0 + self.dr * e);
        cam.normalize();
        cam
    }

    pub fn finished(&self, t: f64) -> bool {
        t >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f64; 2] = [1000.0, 800.0];

    #[test]
    fn arrives_exactly_even_far_away_and_deep() {
        let a = Camera::default();
        let mut b = Camera::default();
        b.zoom_at([10.0, 10.0], VP, 250.0); // ~10^75 deeper, somewhere off-centre
        b.pan_px([1.0e6, -3.0e5]);
        b.rotate_at([500.0, 400.0], VP, 0.4);
        let f = Flight::new(&a, &b, VP);
        let end = f.at(f.duration);
        // Same view: every screen point maps to the same world point.
        for p in [[0.0, 0.0], [999.0, 799.0], [500.0, 400.0]] {
            let wa = end.world_to_screen(&b.screen_to_world(p, VP), VP);
            assert!((wa[0] - p[0]).abs() < 1e-6 && (wa[1] - p[1]).abs() < 1e-6, "{p:?} → {wa:?}");
        }
        assert_eq!(f.at(0.0).center, a.center);
    }

    #[test]
    fn backs_out_to_travel_between_distant_places() {
        // Both views at zoom 2^10 (1024 px per unit), 10 000 units apart: the flight has to zoom
        // out until the distance fits on screen.
        let mut a = Camera::default();
        a.zoom_at([500.0, 400.0], VP, 10.0);
        let mut b = a.clone();
        b.pan_px([-10_000.0 * 1024.0, 0.0]);
        let f = Flight::new(&a, &b, VP);
        let mid = f.at(f.duration * 0.5);
        let fit = (0.6f64 * 800.0 / 10_000.0).log2();
        assert!(mid.log_zoom < fit + 0.5, "mid-flight zoom {} should be ≲ {fit}", mid.log_zoom);
        assert!(f.duration > 1.0 && f.duration <= 4.0);
    }

    #[test]
    fn rotation_takes_the_short_way() {
        let a = Camera::default();
        let mut b = Camera::default();
        b.rotation = -3.0; // ≈ -172°: from 0 the short way is through -90°
        let f = Flight::new(&a, &b, VP);
        let mid = f.at(f.duration * 0.5).rotation;
        assert!(mid < 0.0 && mid > -3.0, "{mid}");
    }
}
