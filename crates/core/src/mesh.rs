//! Stroke → triangle mesh in level-local coordinates (lyon, variable width, round caps).
//!
//! Each vertex is `[x, y, intensity]`: intensity multiplies the ink's alpha (pencil pressure).

use crate::smoothing::{catmull_rom, segments_for_tolerance};
use crate::stroke::{Brush, Stroke, StrokePoint};
use lyon_tessellation::geom::point;
use lyon_tessellation::path::builder::Build;
use lyon_tessellation::path::traits::PathBuilder;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, LineCap, LineJoin, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexBuffers,
};

/// Highest level of detail. LOD `n` is meant to look smooth up to `2^n` × the drawing zoom.
pub const MAX_LOD: u32 = 8;
/// Upper bound on generated vertices for one stroke; LOD is lowered to respect it.
const MAX_VERTICES: usize = 300_000;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// LOD needed when a stroke at `level` is viewed at `log_zoom`.
pub fn lod_for(level: i32, log_zoom: f64) -> u32 {
    (log_zoom - level as f64).ceil().clamp(0.0, MAX_LOD as f64) as u32
}

fn segments(len: f32, lod: u32) -> usize {
    // At LOD 0 one local unit is ~1 px: aim for a sample every ~3 px on screen.
    ((len * (1u32 << lod) as f32) / 3.0).ceil().clamp(1.0, 64.0) as usize
}

pub fn tessellate(stroke: &Stroke, lod: u32) -> Mesh {
    tessellate_part(stroke, 0..stroke.points.len(), lod)
}

/// Tessellates the points `range` of a stroke only (a stroke being drawn is built from
/// frozen parts plus a live tail, so long strokes don't get slower to draw). Interior part
/// ends get round caps; flat caps apply only at the stroke's real ends.
pub fn tessellate_part(stroke: &Stroke, range: std::ops::Range<usize>, lod: u32) -> Mesh {
    if stroke.brush == Brush::Fill && stroke.points[range.clone()].iter().any(|p| p.pressure < 0.0) {
        return tessellate_fill_with_holes(stroke, &stroke.points[range], lod.min(MAX_LOD));
    }
    let mut lod = lod.min(MAX_LOD);
    let caps = [stroke.flat_caps[0] && range.start == 0, stroke.flat_caps[1] && range.end == stroke.points.len()];
    // Raw input is very dense (a sample every ~0.4 px); drop points that don't change the
    // shape by more than a fraction of a pixel at this LOD. Saves most of the geometry.
    let pts = simplify(&dedup(&stroke.points[range]), SIMPLIFY_TOLERANCE / (1u32 << lod) as f32);
    // Lower the LOD if this would produce absurd amounts of geometry.
    while lod > 0 {
        let est: usize = pts
            .windows(2)
            .map(|w| segments(((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt(), lod))
            .sum::<usize>()
            * 12;
        if est <= MAX_VERTICES {
            break;
        }
        lod -= 1;
    }
    let tolerance = 0.2 / (1u32 << lod) as f32;
    let mut out: VertexBuffers<[f32; 3], u32> = VertexBuffers::new();

    if pts.len() == 1 || (stroke.brush == Brush::Fill && pts.len() < 3) {
        let p = pts.last().copied().unwrap_or(StrokePoint { x: 0.0, y: 0.0, pressure: 1.0 });
        if !pts.is_empty() {
            circle(&mut out, p, stroke.radius_at(&p).max(0.025), stroke.intensity(p.pressure), tolerance);
        }
    } else if pts.len() > 1 {
        // Adaptive: straight runs stay one segment, curves get as many as the tolerance needs.
        let smooth = dedup(&smooth_keeping_corners(&pts, tolerance));
        match stroke.brush {
            Brush::Dotted => dots(&mut out, stroke, &smooth, tolerance),
            Brush::Fill => {
                fill_polygon(&mut out, stroke, &smooth, tolerance);
                ribbon(&mut out, stroke, &smooth, tolerance, caps, true);
            }
            _ => ribbon(&mut out, stroke, &smooth, tolerance, caps, false),
        }
    }
    Mesh { vertices: out.vertices, indices: out.indices }
}

/// Turns sharper than this (radians) are kept as corners instead of being smoothed over.
const CORNER_ANGLE: f32 = 1.05;

/// Catmull-Rom smoothing, except at sharp corners: the curve is split there, so corners stay
/// pointed instead of bulging (rectangles stay rectangles, deliberate zigzags stay zigzags).
fn smooth_keeping_corners(pts: &[StrokePoint], tolerance: f32) -> Vec<StrokePoint> {
    let n = pts.len();
    let mut out: Vec<StrokePoint> = Vec::new();
    let mut start = 0;
    for i in 1..n {
        let corner = i + 1 < n && {
            let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
            let d1 = (b.x - a.x, b.y - a.y);
            let d2 = (c.x - b.x, c.y - b.y);
            let cross = d1.0 * d2.1 - d1.1 * d2.0;
            let dot = d1.0 * d2.0 + d1.1 * d2.1;
            cross.atan2(dot).abs() > CORNER_ANGLE
        };
        if corner || i == n - 1 {
            let part = catmull_rom(&pts[start..=i], |_, flatness| segments_for_tolerance(flatness, tolerance));
            // Consecutive parts share their corner point.
            let skip = usize::from(!out.is_empty());
            out.extend(part.into_iter().skip(skip));
            start = i;
        }
    }
    if out.is_empty() {
        out.extend_from_slice(pts);
    }
    out
}

/// Nib angles (local coordinates, y down): calligraphy pen and chisel marker.
const FOUNTAIN_NIB: f32 = -std::f32::consts::FRAC_PI_4;
const MARKER_NIB: f32 = -1.0;

/// Width multiplier of a flat nib for the direction of travel `dir`: thin along the nib.
fn nib(dir: f32, nib_angle: f32, thinnest: f32) -> f32 {
    thinnest + (1.0 - thinnest) * (dir - nib_angle).sin().abs()
}

/// The stroke body: lyon variable-width stroke with per-vertex [width factor, intensity].
fn ribbon(
    out: &mut VertexBuffers<[f32; 3], u32>,
    stroke: &Stroke,
    pts: &[StrokePoint],
    tolerance: f32,
    flat: [bool; 2],
    outline: bool,
) {
    let n = pts.len();
    // Direction of travel over a few neighbours: single segments wobble (esp. at the start,
    // where the input filter settles), and direction-dependent widths would bulge there.
    let dir = |i: usize| {
        let (a, b) = (pts[i.saturating_sub(3)], pts[(i + 3).min(n - 1)]);
        (b.y - a.y).atan2(b.x - a.x)
    };
    let attrs = |i: usize| -> [f32; 2] {
        let p = pts[i];
        let w = if outline { 1.0 } else { stroke.width_factor(p.pressure) };
        let w = match stroke.brush {
            Brush::Fountain => w * nib(dir(i), FOUNTAIN_NIB, 0.2),
            Brush::Marker => w * nib(dir(i), MARKER_NIB, 0.3),
            _ => w,
        };
        [w, stroke.intensity(p.pressure)]
    };
    let cap = |f: bool| if f || stroke.brush == Brush::Marker { LineCap::Butt } else { LineCap::Round };
    let opts = StrokeOptions::tolerance(tolerance)
        .with_line_width(stroke.width.max(0.05))
        .with_variable_line_width(0)
        .with_start_cap(cap(flat[0]))
        .with_end_cap(cap(flat[1]))
        .with_line_join(LineJoin::Round);
    let mut tess = StrokeTessellator::new();
    let mut bb = BuffersBuilder::new(out, |mut v: StrokeVertex| {
        let p = v.position();
        let a = v.interpolated_attributes();
        [p.x, p.y, a[1]]
    });
    let mut b = tess.builder_with_attributes(2, &opts, &mut bb);
    b.begin(point(pts[0].x, pts[0].y), &attrs(0));
    for (i, p) in pts.iter().enumerate().skip(1) {
        b.line_to(point(p.x, p.y), &attrs(i));
    }
    b.end(outline);
    if let Err(e) = b.build() {
        log::warn!("tessellation of stroke {} failed: {e:?}", stroke.id);
    }
}

fn circle(out: &mut VertexBuffers<[f32; 3], u32>, c: StrokePoint, r: f32, intensity: f32, tolerance: f32) {
    let mut fill = FillTessellator::new();
    let _ = fill.tessellate_circle(
        point(c.x, c.y),
        r,
        &FillOptions::tolerance(tolerance.min(r * 0.25)),
        &mut BuffersBuilder::new(out, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y, intensity]
        }),
    );
}

/// Round dots every ~2.2 widths along the path (arc length), the first one at the start.
fn dots(out: &mut VertexBuffers<[f32; 3], u32>, stroke: &Stroke, pts: &[StrokePoint], tolerance: f32) {
    let spacing = (stroke.width * 2.2).max(0.1);
    let mut next = 0.0f32; // arc length of the next dot
    let mut walked = 0.0f32;
    let mut count = 0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        while next <= walked + len && count < 100_000 {
            let t = if len > 0.0 { (next - walked) / len } else { 0.0 };
            let p = StrokePoint { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, pressure: a.pressure + (b.pressure - a.pressure) * t };
            circle(out, p, stroke.radius_at(&p), 1.0, tolerance);
            next += spacing;
            count += 1;
        }
        walked += len;
    }
}

/// A fill made of several outlines (shape + holes), separated by [`Stroke::SUBPATH_BREAK`]
/// points; even-odd rule, so holes stay empty.
fn tessellate_fill_with_holes(stroke: &Stroke, points: &[StrokePoint], lod: u32) -> Mesh {
    let tolerance = 0.2 / (1u32 << lod) as f32;
    let loops: Vec<Vec<StrokePoint>> = points
        .split(|p| p.pressure < 0.0)
        .map(|l| simplify(&dedup(l), SIMPLIFY_TOLERANCE / (1u32 << lod) as f32))
        .filter(|l| l.len() >= 3)
        .map(|l| dedup(&smooth_keeping_corners(&l, tolerance)))
        .collect();
    let mut out: VertexBuffers<[f32; 3], u32> = VertexBuffers::new();
    let mut fill = FillTessellator::new();
    let opts = FillOptions::tolerance(tolerance).with_fill_rule(lyon_tessellation::FillRule::EvenOdd);
    {
        let mut bb = BuffersBuilder::new(&mut out, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y, 1.0]
        });
        let mut b = fill.builder(&opts, &mut bb);
        for l in &loops {
            b.begin(point(l[0].x, l[0].y));
            for p in &l[1..] {
                b.line_to(point(p.x, p.y));
            }
            b.end(true);
        }
        if let Err(e) = b.build() {
            log::warn!("fill of stroke {} failed: {e:?}", stroke.id);
        }
    }
    // A thin outline along every edge closes hairline gaps to the surrounding ink.
    for l in &loops {
        ribbon(&mut out, stroke, l, tolerance, [false; 2], true);
    }
    Mesh { vertices: out.vertices, indices: out.indices }
}

/// The area enclosed by the (implicitly closed) path, non-zero rule.
fn fill_polygon(out: &mut VertexBuffers<[f32; 3], u32>, stroke: &Stroke, pts: &[StrokePoint], tolerance: f32) {
    let mut fill = FillTessellator::new();
    let opts = FillOptions::tolerance(tolerance).with_fill_rule(lyon_tessellation::FillRule::NonZero);
    let mut bb = BuffersBuilder::new(out, |v: FillVertex| {
        let p = v.position();
        [p.x, p.y, 1.0]
    });
    let mut b = fill.builder(&opts, &mut bb);
    b.begin(point(pts[0].x, pts[0].y));
    for p in &pts[1..] {
        b.line_to(point(p.x, p.y));
    }
    b.end(true);
    if let Err(e) = b.build() {
        log::warn!("fill of stroke {} failed: {e:?}", stroke.id);
    }
}

/// Max deviation (local units, ~px at LOD 0) allowed when simplifying input points.
const SIMPLIFY_TOLERANCE: f32 = 0.25;
/// Max pressure deviation allowed when simplifying.
const SIMPLIFY_PRESSURE: f32 = 0.04;

/// Ramer–Douglas–Peucker on position and pressure: keeps a point if it deviates from the
/// chord by more than `tol` in position or `SIMPLIFY_PRESSURE` in pressure.
pub fn simplify(points: &[StrokePoint], tol: f32) -> Vec<StrokePoint> {
    let n = points.len();
    if n <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let (dx, dy) = (pb.x - pa.x, pb.y - pa.y);
        let len2 = dx * dx + dy * dy;
        let mut worst = (0.0f32, 0usize);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let t = if len2 > 0.0 { (((p.x - pa.x) * dx + (p.y - pa.y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (cx, cy) = (pa.x + dx * t, pa.y + dy * t);
            let d = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt() / tol;
            let ti = (i - a) as f32 / (b - a) as f32;
            let dp = (p.pressure - (pa.pressure + (pb.pressure - pa.pressure) * ti)).abs() / SIMPLIFY_PRESSURE;
            let e = d.max(dp);
            if e > worst.0 {
                worst = (e, i);
            }
        }
        if worst.0 > 1.0 {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    points.iter().zip(keep).filter_map(|(p, k)| k.then_some(*p)).collect()
}

/// Removes consecutive points closer than a tiny epsilon (lyon dislikes zero-length edges).
fn dedup(points: &[StrokePoint]) -> Vec<StrokePoint> {
    let mut out: Vec<StrokePoint> = Vec::with_capacity(points.len());
    for p in points {
        if let Some(last) = out.last_mut()
            && (last.x - p.x).abs() < 1e-3
            && (last.y - p.y).abs() < 1e-3
        {
            last.pressure = last.pressure.max(p.pressure);
            continue;
        }
        out.push(*p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigInt;

    fn stroke(n: usize) -> Stroke {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 6.0);
        for i in 0..n {
            let t = i as f32 * 0.3;
            s.push(StrokePoint { x: t.cos() * 50.0 + i as f32, y: t.sin() * 50.0, pressure: (i % 7) as f32 / 7.0 });
        }
        s
    }

    #[test]
    fn mesh_inside_bbox() {
        let s = stroke(40);
        for lod in [0, 3, MAX_LOD] {
            let m = tessellate(&s, lod);
            assert!(!m.is_empty());
            assert_eq!(m.indices.len() % 3, 0);
            let bb = s.cache.bbox;
            for v in &m.vertices {
                assert!(v[0] >= bb[0] - 3.0 && v[0] <= bb[2] + 3.0 && v[1] >= bb[1] - 3.0 && v[1] <= bb[3] + 3.0);
            }
        }
    }

    #[test]
    fn higher_lod_has_more_geometry() {
        let s = stroke(40);
        assert!(tessellate(&s, 6).vertices.len() > tessellate(&s, 0).vertices.len());
    }

    #[test]
    fn single_dot() {
        let s = stroke(1);
        assert!(!tessellate(&s, 0).is_empty());
    }

    #[test]
    fn dense_input_is_simplified() {
        // Straight swipe sampled every 0.4 units with constant pressure, then a dense arc.
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 3.0);
        for i in 0..1250 {
            s.push(StrokePoint { x: i as f32 * 0.4, y: 0.0, pressure: 0.5 });
        }
        for i in 0..400 {
            let a = i as f32 / 400.0 * std::f32::consts::PI;
            s.push(StrokePoint { x: 500.0 + a.sin() * 80.0, y: 80.0 - a.cos() * 80.0, pressure: 0.5 });
        }
        let simple = simplify(&s.points, 0.25);
        assert!(simple.len() < 60, "{}", simple.len());
        let m = tessellate(&s, 0);
        eprintln!("points {} -> {}, triangles {}", s.points.len(), simple.len(), m.indices.len() / 3);
        assert!(m.indices.len() / 3 < 1500, "{}", m.indices.len() / 3);
        // Every original point stays within tolerance of the simplified polyline.
        for p in &s.points {
            let d = simple
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                    ((p.x - a.x - dx * t).powi(2) + (p.y - a.y - dy * t).powi(2)).sqrt()
                })
                .fold(f32::MAX, f32::min);
            assert!(d <= 0.2501, "{d}");
        }
    }

    /// Area covered by a mesh (sum of triangle areas; overlaps counted twice).
    fn area(m: &Mesh) -> f32 {
        m.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
                0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs()
            })
            .sum()
    }

    /// Does the mesh cover point `p`?
    fn covers(m: &Mesh, p: [f32; 2]) -> bool {
        m.indices.as_chunks::<3>().0.iter().any(|t| {
            let v = [m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]];
            let s = |a: [f32; 3], b: [f32; 3]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            let (d0, d1, d2) = (s(v[0], v[1]), s(v[1], v[2]), s(v[2], v[0]));
            (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
        })
    }

    fn line(brush: Brush, from: [f32; 2], to: [f32; 2], pressure: f32) -> Stroke {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 10.0);
        s.brush = brush;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            s.push(StrokePoint { x: from[0] + (to[0] - from[0]) * t, y: from[1] + (to[1] - from[1]) * t, pressure });
        }
        s
    }

    #[test]
    fn every_brush_makes_a_mesh_inside_its_bbox() {
        for brush in Brush::ALL {
            let mut s = stroke(40);
            s.brush = brush;
            s.refresh_cache();
            let m = tessellate(&s, 2);
            assert!(!m.is_empty(), "{brush:?}");
            let bb = s.cache.bbox;
            for v in &m.vertices {
                assert!(v[0] >= bb[0] - 0.5 && v[0] <= bb[2] + 0.5 && v[1] >= bb[1] - 0.5 && v[1] <= bb[3] + 0.5, "{brush:?}");
                assert!(v[2] > 0.0 && v[2] <= 1.0, "{brush:?} intensity");
            }
        }
    }

    #[test]
    fn fountain_pen_follows_direction() {
        // Nib at -45°: moving up-right (along the nib) is thin, down-right is thick.
        let along = tessellate(&line(Brush::Fountain, [0.0, 100.0], [100.0, 0.0], 1.0), 0);
        let across = tessellate(&line(Brush::Fountain, [0.0, 0.0], [100.0, 100.0], 1.0), 0);
        assert!(area(&across) > 3.0 * area(&along), "{} vs {}", area(&across), area(&along));
    }

    #[test]
    fn pencil_pressure_sets_intensity() {
        let light = tessellate(&line(Brush::SoftPencil, [0.0, 0.0], [100.0, 0.0], 0.1), 0);
        let hard = tessellate(&line(Brush::SoftPencil, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        let avg = |m: &Mesh| m.vertices.iter().map(|v| v[2]).sum::<f32>() / m.vertices.len() as f32;
        assert!(avg(&light) < 0.45 && avg(&hard) > 0.95);
    }

    #[test]
    fn dotted_spacing() {
        // 100 units long, width 10 → a dot every 22 units: at 0, 22, 44, 66, 88.
        let m = tessellate(&line(Brush::Dotted, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        for x in [0.0, 22.0, 44.0, 66.0, 88.0] {
            assert!(covers(&m, [x, 0.0]), "dot at {x}");
        }
        assert!(!covers(&m, [11.0, 0.0]) && !covers(&m, [99.0, 0.0]));
    }

    #[test]
    fn fill_covers_the_enclosed_area() {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 2.0);
        s.brush = Brush::Fill;
        for i in 0..64 {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            s.push(StrokePoint { x: 50.0 * a.cos(), y: 50.0 * a.sin(), pressure: 1.0 });
        }
        let m = tessellate(&s, 0);
        assert!(covers(&m, [0.0, 0.0]) && covers(&m, [30.0, -20.0]));
        assert!(!covers(&m, [60.0, 0.0]));
    }

    #[test]
    fn constant_intensity_stays_constant() {
        for brush in [Brush::Marker, Brush::Pen, Brush::Fountain] {
            let mut s = stroke(40);
            s.brush = brush;
            let m = tessellate(&s, 2);
            let bad: Vec<f32> = m.vertices.iter().map(|v| v[2]).filter(|i| (i - 1.0).abs() > 1e-4).collect();
            assert!(bad.is_empty(), "{brush:?}: {:?}", &bad[..bad.len().min(8)]);
        }
    }

    #[test]
    fn marker_has_flat_ends() {
        // A round cap would reach 5 units past the end along the line; a flat one doesn't.
        let m = tessellate(&line(Brush::Marker, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        assert!(!covers(&m, [102.0, 0.0]) && covers(&m, [98.0, 0.0]));
    }

    #[test]
    fn square_fill_keeps_its_corners() {
        // A 100×100 square: with plain Catmull-Rom the edges would bulge past the corners.
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 0.1);
        s.brush = Brush::Fill;
        for p in [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0], [0.0, 0.0]] {
            s.push(StrokePoint { x: p[0], y: p[1], pressure: 1.0 });
        }
        let m = tessellate(&s, 0);
        for v in &m.vertices {
            assert!(v[0] > -0.2 && v[0] < 100.2 && v[1] > -0.2 && v[1] < 100.2, "{v:?}");
        }
        assert!(covers(&m, [99.0, 1.0]) && covers(&m, [1.0, 99.0]), "corners filled");
    }

    #[test]
    fn fill_with_a_hole() {
        let mut s = Stroke::new(1, 0, [BigInt::from(0), BigInt::from(0)], [0, 0, 0, 255], 0.1);
        s.brush = Brush::Fill;
        let sq = |a: f32, b: f32| [[a, a], [b, a], [b, b], [a, b]];
        for p in sq(0.0, 100.0) {
            s.push(StrokePoint { x: p[0], y: p[1], pressure: 1.0 });
        }
        s.push(StrokePoint { x: 0.0, y: 0.0, pressure: Stroke::SUBPATH_BREAK });
        for p in sq(40.0, 60.0) {
            s.push(StrokePoint { x: p[0], y: p[1], pressure: 1.0 });
        }
        let m = tessellate(&s, 0);
        assert!(covers(&m, [10.0, 10.0]) && covers(&m, [90.0, 50.0]));
        assert!(!covers(&m, [50.0, 50.0]), "the hole stays empty");
    }
}
