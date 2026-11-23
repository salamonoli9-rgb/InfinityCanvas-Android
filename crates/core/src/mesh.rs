//! Stroke → triangle mesh in level-local coordinates (lyon, variable width, round caps).

use crate::smoothing::{catmull_rom, segments_for_tolerance};
use crate::stroke::{Stroke, StrokePoint, pressure_factor};
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
    pub vertices: Vec<[f32; 2]>,
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
    let mut out: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();

    if pts.len() == 1 {
        let p = pts[0];
        let r = stroke.radius_at(&p).max(0.05);
        let mut fill = FillTessellator::new();
        let _ = fill.tessellate_circle(
            point(p.x, p.y),
            r,
            &FillOptions::tolerance(tolerance),
            &mut BuffersBuilder::new(&mut out, |v: FillVertex| v.position().to_array()),
        );
    } else if pts.len() > 1 {
        // Adaptive: straight runs stay one segment, curves get as many as the tolerance needs.
        let smooth = catmull_rom(&pts, |_, flatness| segments_for_tolerance(flatness, tolerance));
        let smooth = dedup(&smooth);
        let opts = StrokeOptions::tolerance(tolerance)
            .with_line_width(stroke.width.max(0.05))
            .with_variable_line_width(0)
            .with_start_cap(if caps[0] { LineCap::Butt } else { LineCap::Round })
            .with_end_cap(if caps[1] { LineCap::Butt } else { LineCap::Round })
            .with_line_join(LineJoin::Round);
        let mut tess = StrokeTessellator::new();
        let mut bb = BuffersBuilder::new(&mut out, |v: StrokeVertex| v.position().to_array());
        let mut b = tess.builder_with_attributes(1, &opts, &mut bb);
        let first = smooth[0];
        b.begin(point(first.x, first.y), &[pressure_factor(first.pressure)]);
        for p in &smooth[1..] {
            b.line_to(point(p.x, p.y), &[pressure_factor(p.pressure)]);
        }
        b.end(false);
        if let Err(e) = b.build() {
            log::warn!("tessellation of stroke {} failed: {e:?}", stroke.id);
        }
    }
    Mesh { vertices: out.vertices, indices: out.indices }
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
}
