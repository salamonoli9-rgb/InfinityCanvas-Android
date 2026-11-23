//! The measuring tool: canvas length and angle of a dragged line.

/// Length of a line `screen_len` px long on screen, in points at 100 % ("Początek" zoom).
/// Very large / small lengths are written in powers of ten.
pub fn length_label(screen_len: f64, log_zoom: f64, ppp: f64) -> String {
    if screen_len <= 0.0 {
        return "0 pt".into();
    }
    let log10 = screen_len.log10() - log_zoom * std::f64::consts::LOG10_2 - ppp.log10();
    if (-2.0..6.0).contains(&log10) {
        let v = 10f64.powf(log10);
        let digits = if v >= 100.0 { 0 } else if v >= 10.0 { 1 } else { 2 };
        format!("{} pt", format!("{v:.digits$}").replace('.', ","))
    } else {
        let e = log10.floor();
        let m = 10f64.powf(log10 - e);
        format!("{}·10^{} pt", format!("{m:.1}").replace('.', ","), e as i64)
    }
}

/// Angle of the line from `a` to `b` on the canvas (screen angle minus the canvas rotation),
/// in degrees, 0° = to the right, counter-clockwise positive like on paper (y up).
pub fn angle_deg(a: [f64; 2], b: [f64; 2], rotation: f64) -> f64 {
    let screen = -(b[1] - a[1]).atan2(b[0] - a[0]);
    let deg = (screen + rotation).to_degrees();
    let d = deg.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(length_label(150.0, 0.0, 1.0), "150 pt");
        assert_eq!(length_label(150.0, 1.0, 1.0), "75,0 pt");
        assert_eq!(length_label(300.0, 0.0, 3.0), "100 pt");
        assert_eq!(length_label(100.0, 100.0, 1.0), "7,9·10^-29 pt");
        assert_eq!(length_label(100.0, -100.0, 1.0), "1,3·10^32 pt");
    }

    #[test]
    fn angles() {
        assert!((angle_deg([0.0, 0.0], [10.0, 0.0], 0.0)).abs() < 1e-9);
        assert!((angle_deg([0.0, 0.0], [0.0, -10.0], 0.0) - 90.0).abs() < 1e-9, "up on screen = 90°");
        // The canvas turned by 90°: a horizontal screen line runs along the canvas' vertical.
        assert!((angle_deg([0.0, 0.0], [10.0, 0.0], std::f64::consts::FRAC_PI_2) - 90.0).abs() < 1e-9);
    }
}
