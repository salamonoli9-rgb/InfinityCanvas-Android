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

/// Centripetal-free uniform Catmull-Rom subdivision of a polyline.
///
/// `segments_for(len)` decides how many pieces each input segment becomes.
pub fn catmull_rom(points: &[StrokePoint], segments_for: impl Fn(f32) -> usize) -> Vec<StrokePoint> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let n = points.len();
    let get = |i: isize| points[i.clamp(0, n as isize - 1) as usize];
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (get(i as isize - 1), get(i as isize), get(i as isize + 1), get(i as isize + 2));
        let len = ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2)).sqrt();
        let segs = segments_for(len).max(1);
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
        let out = catmull_rom(&pts, |_| 4);
        assert_eq!(out.len(), 4 * 4 + 1);
        for (i, p) in pts.iter().enumerate() {
            let q = out[i * 4];
            assert!((q.x - p.x).abs() < 1e-4 && (q.y - p.y).abs() < 1e-4);
        }
    }
}
