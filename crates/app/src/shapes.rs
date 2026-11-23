//! Shape guides: straight lines, rectangles and ellipses dragged from corner to corner.
//! Geometry is built in screen space (so shapes follow the screen axes even on a rotated
//! canvas) and converted to stroke points by the caller.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Line,
    Rectangle,
    Ellipse,
}

impl Shape {
    pub const ALL: [Shape; 3] = [Shape::Line, Shape::Rectangle, Shape::Ellipse];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Line => "Linia",
            Shape::Rectangle => "Prostokąt",
            Shape::Ellipse => "Elipsa",
        }
    }
}

/// Line angles within this of a multiple of 45° snap to it (radians, ~4°).
const ANGLE_SNAP: f64 = 0.07;
/// Rectangles / ellipses whose sides differ by less than this fraction become squares / circles.
const SQUARE_SNAP: f64 = 0.08;

fn snap_square(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let m = dx.abs().max(dy.abs());
    if m > 0.0 && (dx.abs() - dy.abs()).abs() < SQUARE_SNAP * m {
        [a[0] + m * dx.signum(), a[1] + m * dy.signum()]
    } else {
        b
    }
}

/// Points (screen px) of `shape` dragged from `a` to `b`. Closed shapes repeat the first point.
pub fn points(shape: Shape, a: [f64; 2], b: [f64; 2]) -> Vec<[f64; 2]> {
    match shape {
        Shape::Line => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx.hypot(dy);
            let ang = dy.atan2(dx);
            let step = std::f64::consts::FRAC_PI_4;
            let snapped = (ang / step).round() * step;
            let b = if (ang - snapped).abs() < ANGLE_SNAP { [a[0] + len * snapped.cos(), a[1] + len * snapped.sin()] } else { b };
            vec![a, b]
        }
        Shape::Rectangle => {
            let b = snap_square(a, b);
            vec![a, [b[0], a[1]], b, [a[0], b[1]], a]
        }
        Shape::Ellipse => {
            let b = snap_square(a, b);
            let c = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let r = [(b[0] - a[0]).abs() * 0.5, (b[1] - a[1]).abs() * 0.5];
            let n = 96;
            (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64 * std::f64::consts::TAU;
                    [c[0] + r[0] * t.cos(), c[1] + r[1] * t.sin()]
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_snaps_to_45_degrees() {
        let p = points(Shape::Line, [0.0, 0.0], [100.0, 4.0]);
        assert!((p[1][1]).abs() < 1e-9 && (p[1][0] - 100.08).abs() < 0.01, "{:?}", p[1]);
        let p = points(Shape::Line, [0.0, 0.0], [100.0, 30.0]);
        assert_eq!(p[1], [100.0, 30.0], "no snap far from 45° multiples");
    }

    #[test]
    fn near_square_becomes_square_and_circle() {
        let r = points(Shape::Rectangle, [10.0, 10.0], [110.0, 104.0]);
        assert_eq!(r[2], [110.0, 110.0]);
        assert_eq!(r.first(), r.last());
        let e = points(Shape::Ellipse, [0.0, 0.0], [-50.0, 48.0]);
        for p in &e {
            let d = ((p[0] + 25.0).powi(2) + (p[1] - 25.0).powi(2)).sqrt();
            assert!((d - 25.0).abs() < 1e-9);
        }
        let flat = points(Shape::Rectangle, [0.0, 0.0], [100.0, 40.0]);
        assert_eq!(flat[2], [100.0, 40.0]);
    }
}
