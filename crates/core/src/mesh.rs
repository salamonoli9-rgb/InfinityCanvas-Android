//! Stroke → triangle mesh in level-local coordinates (lyon, variable width, round caps).

use crate::smoothing::catmull_rom;
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
    let mut lod = lod.min(MAX_LOD);
    let pts = dedup(&stroke.points);
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
        let smooth = catmull_rom(&pts, |len| segments(len, lod));
        let smooth = dedup(&smooth);
        let opts = StrokeOptions::tolerance(tolerance)
            .with_line_width(stroke.width.max(0.05))
            .with_variable_line_width(0)
            .with_line_cap(LineCap::Round)
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
}
