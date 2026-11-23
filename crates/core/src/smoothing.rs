//! Input smoothing: One Euro filter (jitter removal with low lag) and Catmull-Rom
//! interpolation (smooth curves through the filtered samples).

use crate::stroke::StrokePoint;

/// One Euro filter for a single scalar. See Casiez et al., CHI 2012.
#[derive(Clone, Debug)]
pub struct OneEuro {
    min_cutoff: f64,
    beta: f64,
    d_cutoff: f64,
    x: Option<f64>,
    dx: f64,
}

fn alpha(cutoff: f64, dt: f64) -> f64 {
    let tau = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

impl OneEuro {
    pub fn new(min_cutoff: f64, beta: f64) -> Self {
        Self { min_cutoff, beta, d_cutoff: 1.0, x: None, dx: 0.0 }
    }

    pub fn filter(&mut self, value: f64, dt: f64) -> f64 {
        let dt = dt.max(1e-4);
        let Some(prev) = self.x else {
            self.x = Some(value);
            return value;
        };
        let d = (value - prev) / dt;
        let ad = alpha(self.d_cutoff, dt);
        self.dx = ad * d + (1.0 - ad) * self.dx;
        let cutoff = self.min_cutoff + self.beta * self.dx.abs();
        let a = alpha(cutoff, dt);
        let x = a * value + (1.0 - a) * prev;
        self.x = Some(x);
        x
    }
}

/// Smooths raw pointer samples (in screen pixels) of one stroke.
#[derive(Clone, Debug)]
pub struct PointerSmoother {
    fx: OneEuro,
    fy: OneEuro,
    fp: OneEuro,
    last_t: Option<f64>,
}

impl Default for PointerSmoother {
    fn default() -> Self {
        // Tuned for pixel units: strong smoothing when slow, little lag when fast.
        Self { fx: OneEuro::new(1.5, 0.02), fy: OneEuro::new(1.5, 0.02), fp: OneEuro::new(2.0, 0.0), last_t: None }
    }
}

impl PointerSmoother {
    /// `t` in seconds. Returns the filtered (x, y, pressure).
    pub fn push(&mut self, x: f64, y: f64, pressure: f64, t: f64) -> (f64, f64, f64) {
        let dt = match self.last_t {
            Some(lt) if t > lt => t - lt,
            Some(_) => 1.0 / 240.0,
            None => 1.0 / 120.0,
        };
        self.last_t = Some(t);
        (self.fx.filter(x, dt), self.fy.filter(y, dt), self.fp.filter(pressure, dt))
    }
}

/// Uniform Catmull-Rom subdivision of a polyline.
///
/// `segments_for(len, flatness)` decides how many pieces each input segment becomes; `flatness`
/// is the second-difference bound `L` of the segment's Bézier form, so splitting into `n`
/// pieces deviates from the true curve by at most `0.75 · L / n²` (see [`segments_for_tolerance`]).
pub fn catmull_rom(points: &[StrokePoint], segments_for: impl Fn(f32, f32) -> usize) -> Vec<StrokePoint> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let n = points.len();
    let get = |i: isize| points[i.clamp(0, n as isize - 1) as usize];
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (get(i as isize - 1), get(i as isize), get(i as isize + 1), get(i as isize + 2));
        let len = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
        // Bézier control points of this Catmull-Rom segment.
        let b1 = (p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0);
        let b2 = (p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0);
        let d1 = (p1.x - 2.0 * b1.0 + b2.0, p1.y - 2.0 * b1.1 + b2.1);
        let d2 = (b1.0 - 2.0 * b2.0 + p2.x, b1.1 - 2.0 * b2.1 + p2.y);
        let flatness = (d1.0.hypot(d1.1)).max(d2.0.hypot(d2.1));
        let segs = segments_for(len, flatness).max(1);
        for s in 0..segs {
            let t = s as f32 / segs as f32;
            let (t2, t3) = (t * t, t * t * t);
            let cr = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * ((2.0 * b) + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
            };
            out.push(StrokePoint {
                x: cr(p0.x, p1.x, p2.x, p3.x),
                y: cr(p0.y, p1.y, p2.y, p3.y),
                pressure: p1.pressure + (p2.pressure - p1.pressure) * t,
            });
        }
    }
    out.push(points[n - 1]);
    out
}

/// Pieces needed so that a Catmull-Rom segment with the given `flatness` stays within `tol`.
pub fn segments_for_tolerance(flatness: f32, tol: f32) -> usize {
    (0.75 * flatness / tol).sqrt().ceil().clamp(1.0, 64.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_euro_reduces_jitter() {
        let mut f = OneEuro::new(1.0, 0.0);
        let mut max_dev: f64 = 0.0;
        for i in 0..500 {
            let noise = if i % 2 == 0 { 1.0 } else { -1.0 };
            let v = f.filter(100.0 + noise, 1.0 / 120.0);
            if i > 100 {
                max_dev = max_dev.max((v - 100.0).abs());
            }
        }
        assert!(max_dev < 0.2, "{max_dev}");
    }

    #[test]
    fn catmull_rom_passes_through_points() {
        let pts: Vec<StrokePoint> =
            (0..5).map(|i| StrokePoint { x: i as f32 * 10.0, y: (i % 2) as f32 * 10.0, pressure: 0.5 }).collect();
        let out = catmull_rom(&pts, |_, _| 4);
        assert_eq!(out.len(), 4 * 4 + 1);
        for (i, p) in pts.iter().enumerate() {
            let q = out[i * 4];
            assert!((q.x - p.x).abs() < 1e-4 && (q.y - p.y).abs() < 1e-4);
        }
    }

    #[test]
    fn adaptive_subdivision_meets_tolerance() {
        // Coarse points on a circle of radius 100 (sharp-ish curvature) plus a straight run.
        let mut pts: Vec<StrokePoint> =
            (0..12).map(|i| {
                let a = i as f32 * 0.5;
                StrokePoint { x: 100.0 * a.cos(), y: 100.0 * a.sin(), pressure: 1.0 }
            }).collect();
        pts.extend((1..5).map(|i| StrokePoint { x: 100.0 * 5.5f32.cos() + i as f32 * 80.0, y: 100.0 * 5.5f32.sin(), pressure: 1.0 }));
        let tol = 0.2;
        let coarse = catmull_rom(&pts, |_, f| segments_for_tolerance(f, tol));
        let fine = catmull_rom(&pts, |_, _| 64);
        assert!(coarse.len() < fine.len() / 3, "{} vs {}", coarse.len(), fine.len());
        // Every point of the (nearly exact) fine curve is within tolerance of the coarse polyline.
        for p in &fine {
            let d = coarse
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let l2 = dx * dx + dy * dy;
                    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    ((p.x - a.x - dx * t).powi(2) + (p.y - a.y - dy * t).powi(2)).sqrt()
                })
                .fold(f32::MAX, f32::min);
            assert!(d <= tol * 1.05, "deviation {d}");
        }
    }
}
