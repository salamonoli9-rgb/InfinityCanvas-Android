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
    if stroke.brush == Brush::Spray {
        // From the raw points: the dots must not move when the LOD changes.
        let mut out: VertexBuffers<[f32; 3], u32> = VertexBuffers::new();
        spray(&mut out, stroke, &dedup(&stroke.points[range]));
        return Mesh { vertices: out.vertices, indices: out.indices };
    }
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
            Brush::Brush => {
                // Enough points along the path for the tapered ends.
                let total: f32 = smooth.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt()).sum();
                let dense = densify(&smooth, (total / 48.0).max(stroke.width * 0.5).max(0.05));
                ribbon(&mut out, stroke, &dense, tolerance, caps, false)
            }
            Brush::Airbrush => soft_ribbon(&mut out, stroke, &smooth, tolerance, &AIRBRUSH_PROFILE),
            Brush::Watercolor => soft_ribbon(&mut out, stroke, &smooth, tolerance, &WATERCOLOR_PROFILE),
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

/// Inserts points so that no segment is longer than `max_len`.
fn densify(pts: &[StrokePoint], max_len: f32) -> Vec<StrokePoint> {
    let mut out = Vec::with_capacity(pts.len());
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let n = (len / max_len).ceil().clamp(1.0, 256.0) as usize;
        for i in 0..n {
            let t = i as f32 / n as f32;
            out.push(StrokePoint { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, pressure: a.pressure + (b.pressure - a.pressure) * t });
        }
    }
    if let Some(l) = pts.last() {
        out.push(*l);
    }
    out
}

/// Width factor of a tapered stroke at arc length `s` of `total`: thin at both ends.
fn taper(s: f32, total: f32, width: f32) -> f32 {
    let t = (total * 0.2).min(width * 5.0).max(1e-3);
    let e = (s.min(total - s) / t).clamp(0.0, 1.0);
    0.12 + 0.88 * e.sqrt()
}

/// Intensity across a soft stroke, from the edge (0) through the middle and back: offsets as a
/// fraction of the half width with the intensity there.
const AIRBRUSH_PROFILE: [(f32, f32); 5] = [(-1.0, 0.0), (-0.55, 0.45), (0.0, 1.0), (0.55, 0.45), (1.0, 0.0)];
/// Watercolour: a darker rim (pigment settles at the edge of a wash), lighter inside.
const WATERCOLOR_PROFILE: [(f32, f32); 7] = [(-1.0, 0.0), (-0.85, 0.95), (-0.6, 0.6), (0.0, 0.5), (0.6, 0.6), (0.85, 0.95), (1.0, 0.0)];

/// A stroke with soft edges: our own triangle strip across the path with an intensity profile
/// (lyon's strokes only have vertices on the edges), and soft round ends.
fn soft_ribbon(out: &mut VertexBuffers<[f32; 3], u32>, stroke: &Stroke, pts: &[StrokePoint], tolerance: f32, profile: &[(f32, f32)]) {
    let n = pts.len();
    let k = profile.len() as u32;
    let normal = |i: usize| {
        let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l = dx.hypot(dy).max(1e-6);
        (-dy / l, dx / l)
    };
    let base = out.vertices.len() as u32;
    for (i, p) in pts.iter().enumerate() {
        let r = stroke.radius_at(p);
        let (nx, ny) = normal(i);
        for &(o, a) in profile {
            out.vertices.push([p.x + nx * r * o, p.y + ny * r * o, a]);
        }
    }
    for i in 0..n as u32 - 1 {
        for j in 0..k - 1 {
            let (a, b) = (base + i * k + j, base + (i + 1) * k + j);
            out.indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    // Round ends: a fan from the middle (full intensity) to the rim (zero).
    let middle = profile.iter().min_by(|a, b| a.0.abs().total_cmp(&b.0.abs())).map_or(1.0, |(_, a)| *a);
    for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
        let p = pts[i];
        let r = stroke.radius_at(&p);
        let (nx, ny) = normal(i);
        let (tx, ty) = (ny * -sign, -nx * -sign); // along the path, pointing out of the stroke
        let steps = ((std::f32::consts::PI * r / tolerance.max(0.05)).sqrt().ceil() as u32).clamp(4, 24);
        let c = out.vertices.len() as u32;
        out.vertices.push([p.x, p.y, middle]);
        for s in 0..=steps {
            let t = std::f32::consts::PI * s as f32 / steps as f32;
            let (cs, sn) = (t.cos(), t.sin());
            // From one side (+normal) over the tip to the other side (−normal).
            let dx = nx * cs + tx * sn;
            let dy = ny * cs + ty * sn;
            out.vertices.push([p.x + dx * r, p.y + dy * r, 0.0]);
        }
        for s in 0..steps {
            out.indices.extend_from_slice(&[c, c + 1 + s, c + 2 + s]);
        }
    }
}

/// Spray: dots scattered over the brush disc along the path, from a random generator seeded by
/// the stroke and the position along it (the same dots every time it is tessellated).
fn spray(out: &mut VertexBuffers<[f32; 3], u32>, stroke: &Stroke, pts: &[StrokePoint]) {
    let rng = |seed: u64| {
        // SplitMix64.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f32 / (1u64 << 53) as f32
    };
    let step = (stroke.width * 0.06).max(0.05);
    let mut dot = |c: [f32; 2], r: f32| {
        let b = out.vertices.len() as u32;
        out.vertices.push([c[0], c[1], 1.0]);
        for i in 0..6 {
            let a = i as f32 * std::f32::consts::TAU / 6.0;
            out.vertices.push([c[0] + r * a.cos(), c[1] + r * a.sin(), 1.0]);
        }
        for i in 0..6 {
            out.indices.extend_from_slice(&[b, b + 1 + i, b + 1 + (i + 1) % 6]);
        }
    };
    let seed = stroke.id.wrapping_mul(0x2545_F491_4F6C_DD1D);
    let mut emit = |p: StrokePoint, k: u64| {
        let r = stroke.radius_at(&p);
        for j in 0..3u64 {
            let s = seed ^ (k * 8 + j).wrapping_mul(0x9E37_79B9);
            // Uniform over the disc, denser towards the middle (square root left out on purpose).
            let (a, d) = (rng(s) * std::f32::consts::TAU, rng(s ^ 0xABCD) * r);
            let size = stroke.width * (0.012 + 0.025 * rng(s ^ 0x1234));
            dot([p.x + d * a.cos(), p.y + d * a.sin()], size);
        }
    };
    if pts.len() == 1 {
        for k in 0..12 {
            emit(pts[0], k);
        }
        return;
    }
    let mut next = 0.0f32;
    let mut walked = 0.0f32;
    let mut k = 0u64;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        while next <= walked + len && k < 200_000 {
            let t = if len > 0.0 { (next - walked) / len } else { 0.0 };
            emit(StrokePoint { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, pressure: a.pressure + (b.pressure - a.pressure) * t }, k);
            next += step;
            k += 1;
        }
        walked += len;
    }
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
    let mut arc = Vec::with_capacity(n);
    let mut total = 0.0f32;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            total += ((p.x - pts[i - 1].x).powi(2) + (p.y - pts[i - 1].y).powi(2)).sqrt();
        }
        arc.push(total);
    }
    let attrs = |i: usize| -> [f32; 2] {
        let p = pts[i];
        let w = if outline { 1.0 } else { stroke.width_factor(p.pressure) };
        let w = match stroke.brush {
            Brush::Fountain => w * nib(dir(i), FOUNTAIN_NIB, 0.2),
            Brush::Marker => w * nib(dir(i), MARKER_NIB, 0.3),
            // A paint brush thins out at both ends.
            Brush::Brush if !outline => w * taper(arc[i], total, stroke.width),
            _ => w,
        };
        [w, stroke.intensity(p.pressure)]
    };
    let cap = |f: bool| if f || matches!(stroke.brush, Brush::Marker | Brush::Highlighter) { LineCap::Butt } else { LineCap::Round };
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
                assert!(v[2] >= 0.0 && v[2] <= 1.0, "{brush:?} intensity");
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

    #[test]
    fn new_brushes() {
        // Airbrush: soft (zero intensity at the edges, full in the middle).
        let m = tessellate(&line(Brush::Airbrush, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        let edge = m.vertices.iter().filter(|v| v[1].abs() > 4.9).map(|v| v[2]).fold(0.0, f32::max);
        let mid = m.vertices.iter().filter(|v| v[1].abs() < 0.1).map(|v| v[2]).fold(0.0, f32::max);
        assert!(edge < 0.01 && mid > 0.99, "{edge} {mid}");
        // Watercolour: darker rim than middle.
        let m = tessellate(&line(Brush::Watercolor, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        let rim = m.vertices.iter().filter(|v| (v[1].abs() - 4.25).abs() < 0.1).map(|v| v[2]).fold(0.0, f32::max);
        let inner = m.vertices.iter().filter(|v| v[1].abs() < 0.1 && v[0] > 1.0).map(|v| v[2]).fold(0.0, f32::max);
        assert!(rim > inner, "{rim} {inner}");
        // Brush: thin at the ends, full in the middle.
        let m = tessellate(&line(Brush::Brush, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        let half_width_at = |x: f32| m.vertices.iter().filter(|v| (v[0] - x).abs() < 3.0).map(|v| v[1].abs()).fold(0.0, f32::max);
        assert!(half_width_at(3.0) < 0.6 * half_width_at(50.0), "{} {}", half_width_at(3.0), half_width_at(50.0));
        // Spray: dots within the brush disc, the same ones at every LOD.
        let s = line(Brush::Spray, [0.0, 0.0], [100.0, 0.0], 1.0);
        let (a, b) = (tessellate(&s, 0), tessellate(&s, 5));
        assert_eq!(a.vertices, b.vertices, "LOD-independent");
        assert!(a.vertices.iter().all(|v| v[1].abs() <= 5.5));
        assert!(a.indices.len() / 3 > 100);
        // Highlighter: flat ends.
        let m = tessellate(&line(Brush::Highlighter, [0.0, 0.0], [100.0, 0.0], 1.0), 0);
        assert!(!covers(&m, [102.0, 0.0]) && covers(&m, [98.0, 0.0]));
        for b in [Brush::Ballpoint, Brush::Crayon, Brush::Charcoal] {
            assert!(!tessellate(&line(b, [0.0, 0.0], [100.0, 0.0], 0.5), 0).is_empty());
        }
    }
}
