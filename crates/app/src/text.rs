//! Vector text: glyph outlines of the UI font (Ubuntu, with Polish letters) flattened into
//! closed loops, to be filled even-odd (so the holes of "o", "ą", "B" stay open).

/// Outline loops (screen px) of `text` with its top-left at `origin`, `size` px tall (em).
/// Lines are separated by '\n'.
pub fn outline(text: &str, size: f64, origin: [f64; 2]) -> Vec<Vec<[f64; 2]>> {
    let defs = egui::FontDefinitions::default();
    let Some(data) = defs.font_data.get("Ubuntu-Light") else { return Vec::new() };
    let bytes: &[u8] = (*data.font).as_ref();
    let Ok(face) = ttf_parser::Face::parse(bytes, data.index) else { return Vec::new() };
    let k = size / face.units_per_em() as f64;
    let ascender = face.ascender() as f64 * k;
    let line_height = (face.ascender() - face.descender() + face.line_gap()) as f64 * k;
    let mut loops = Vec::new();
    for (row, line) in text.split('\n').enumerate() {
        let baseline = origin[1] + ascender + row as f64 * line_height;
        let mut x = origin[0];
        for c in line.chars() {
            let Some(g) = face.glyph_index(c) else {
                x += size * 0.5;
                continue;
            };
            let mut b = Flatten { loops: &mut loops, cur: Vec::new(), k, x, y: baseline, tol: (size / 200.0).max(0.05) };
            face.outline_glyph(g, &mut b);
            b.close();
            x += face.glyph_hor_advance(g).unwrap_or(0) as f64 * k;
        }
    }
    loops.retain(|l| l.len() >= 3);
    loops
}

/// Collects a glyph outline as polylines (curves subdivided), mapped to screen px.
struct Flatten<'a> {
    loops: &'a mut Vec<Vec<[f64; 2]>>,
    cur: Vec<[f64; 2]>,
    k: f64,
    x: f64,
    y: f64,
    /// Max deviation of the polyline from the curve, px.
    tol: f64,
}

impl Flatten<'_> {
    fn map(&self, x: f32, y: f32) -> [f64; 2] {
        // Font units: y up; screen: y down.
        [self.x + x as f64 * self.k, self.y - y as f64 * self.k]
    }

    fn last(&self) -> [f64; 2] {
        *self.cur.last().unwrap_or(&[self.x, self.y])
    }

    fn segments(&self, a: [f64; 2], ctrl: &[[f64; 2]], b: [f64; 2]) -> usize {
        // Distance of the control points from the chord bounds the flattening error.
        let d = ctrl.iter().map(|c| (c[0] - (a[0] + b[0]) * 0.5).hypot(c[1] - (a[1] + b[1]) * 0.5)).fold(0.0, f64::max);
        ((d / self.tol).sqrt().ceil() as usize).clamp(1, 32)
    }

    fn close(&mut self) {
        if self.cur.len() >= 3 {
            self.loops.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

impl ttf_parser::OutlineBuilder for Flatten<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        let p = self.map(x, y);
        self.cur.push(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.cur.push(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, c, b) = (self.last(), self.map(x1, y1), self.map(x, y));
        let n = self.segments(a, &[c], b);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            self.cur.push([u * u * a[0] + 2.0 * u * t * c[0] + t * t * b[0], u * u * a[1] + 2.0 * u * t * c[1] + t * t * b[1]]);
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, c1, c2, b) = (self.last(), self.map(x1, y1), self.map(x2, y2), self.map(x, y));
        let n = self.segments(a, &[c1, c2], b);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            self.cur.push([
                w[0] * a[0] + w[1] * c1[0] + w[2] * c2[0] + w[3] * b[0],
                w[0] * a[1] + w[1] * c1[1] + w[2] * c2[1] + w[3] * b[1],
            ]);
        }
    }

    fn close(&mut self) {
        Flatten::close(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_have_outlines_and_holes() {
        // "o": an outer and an inner loop (the hole). Polish "ą" and "ł" are there too.
        assert_eq!(outline("o", 100.0, [0.0, 0.0]).len(), 2);
        assert!(outline("ą", 100.0, [0.0, 0.0]).len() >= 2);
        assert!(!outline("ł", 100.0, [0.0, 0.0]).is_empty());
        // Placement: everything right of / below the origin, roughly one em tall.
        let l = outline("Ab", 100.0, [50.0, 20.0]);
        let (mut x0, mut y0, mut y1) = (f64::MAX, f64::MAX, f64::MIN);
        for p in l.iter().flatten() {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        assert!(x0 >= 50.0 && y0 >= 20.0 && y1 - y0 < 110.0, "{x0} {y0} {y1}");
        // A second line goes below the first.
        let two = outline("a\na", 100.0, [0.0, 0.0]);
        let max_y = two.iter().flatten().map(|p| p[1]).fold(f64::MIN, f64::max);
        assert!(max_y > 150.0);
        assert!(outline(" ", 100.0, [0.0, 0.0]).is_empty());
    }
}
