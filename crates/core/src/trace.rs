//! Photo → line drawing: Canny edge detection, then edge pixels chained into polylines.
//!
//! Works on a grayscale image (row-major, one byte per pixel). The result are polylines in
//! pixel coordinates, simplified, long enough to look like deliberate strokes.

use crate::mesh::simplify;
use crate::stroke::StrokePoint;

#[derive(Clone, Copy, Debug)]
pub struct TraceOptions {
    /// 0 = only the strongest outlines, 1 = lots of detail.
    pub detail: f32,
}

/// Separable Gaussian blur.
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let pass = |src: &[f32], horizontal: bool| {
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let d = i as isize - r;
                    let (sx, sy) = if horizontal {
                        ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                    };
                    acc += src[sy * w + sx] * kv;
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

/// Canny edges: a boolean map of thin edge pixels.
pub fn edges(gray: &[u8], w: usize, h: usize, opts: TraceOptions) -> Vec<bool> {
    assert_eq!(gray.len(), w * h);
    let detail = opts.detail.clamp(0.0, 1.0);
    let img: Vec<f32> = gray.iter().map(|&v| v as f32).collect();
    let img = blur(&img, w, h, 1.0 + 1.6 * (1.0 - detail));
    // Sobel.
    let mut mag = vec![0.0f32; w * h];
    let mut dir = vec![0u8; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let p = |dx: isize, dy: isize| img[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let gx = -p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1) + p(1, -1) + 2.0 * p(1, 0) + p(1, 1);
            let gy = -p(-1, -1) - 2.0 * p(0, -1) - p(1, -1) + p(-1, 1) + 2.0 * p(0, 1) + p(1, 1);
            mag[y * w + x] = (gx * gx + gy * gy).sqrt();
            // Gradient direction quantised to 0°, 45°, 90°, 135°.
            let a = gy.atan2(gx).to_degrees().rem_euclid(180.0);
            dir[y * w + x] = (((a + 22.5) / 45.0) as u8) % 4;
        }
    }
    // Non-maximum suppression.
    let mut thin = vec![0.0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            let (dx, dy): (isize, isize) = match dir[i] {
                0 => (1, 0),
                1 => (1, 1),
                2 => (0, 1),
                _ => (-1, 1),
            };
            let a = mag[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let b = mag[(y as isize - dy) as usize * w + (x as isize - dx) as usize];
            if mag[i] >= a && mag[i] >= b {
                thin[i] = mag[i];
            }
        }
    }
    // Thresholds from the distribution of edge strengths: keep the strongest few percent.
    let mut strengths: Vec<f32> = thin.iter().copied().filter(|v| *v > 1.0).collect();
    if strengths.is_empty() {
        return vec![false; w * h];
    }
    strengths.sort_by(f32::total_cmp);
    let keep = 0.06 + 0.24 * detail; // fraction of edge candidates that count as strong
    let high = strengths[((1.0 - keep) * (strengths.len() - 1) as f32) as usize].max(20.0);
    let low = high * 0.45;
    // Hysteresis: weak pixels survive when connected to strong ones.
    let mut out = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h).filter(|&i| thin[i] >= high).collect();
    for &i in &stack {
        out[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if !out[j] && thin[j] >= low {
                    out[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    out
}

const NEIGHBOURS: [(isize, isize); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Chains edge pixels into polylines (pixel coordinates), longest first.
pub fn chains(edge: &[bool], w: usize, h: usize, min_len: usize) -> Vec<Vec<[f32; 2]>> {
    let mut left = edge.to_vec();
    let neighbours = |left: &[bool], i: usize| -> Vec<usize> {
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        NEIGHBOURS
            .iter()
            .filter_map(|(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                (nx >= 0 && ny >= 0 && nx < w as isize && ny < h as isize && left[ny as usize * w + nx as usize])
                    .then_some(ny as usize * w + nx as usize)
            })
            .collect()
    };
    // Start from line ends first (one neighbour), then whatever is left (closed loops).
    let mut starts: Vec<usize> = (0..w * h).filter(|&i| left[i] && neighbours(&left, i).len() == 1).collect();
    starts.extend((0..w * h).filter(|&i| left[i]));
    let mut out = Vec::new();
    for s in starts {
        if !left[s] {
            continue;
        }
        let mut chain = vec![s];
        left[s] = false;
        let mut cur = s;
        let mut heading: Option<(isize, isize)> = None;
        loop {
            let ns = neighbours(&left, cur);
            if ns.is_empty() {
                break;
            }
            // Prefer going straight on.
            let (cx, cy) = ((cur % w) as isize, (cur / w) as isize);
            let next = *ns
                .iter()
                .max_by_key(|&&n| {
                    let d = ((n % w) as isize - cx, (n / w) as isize - cy);
                    let straight = heading.map_or(0, |h| h.0 * d.0 + h.1 * d.1);
                    // Orthogonal steps first among equals: they keep chains 1 px thin.
                    straight * 4 + if d.0 == 0 || d.1 == 0 { 1 } else { 0 }
                })
                .expect("non-empty");
            heading = Some(((next % w) as isize - cx, (next / w) as isize - cy));
            left[next] = false;
            chain.push(next);
            cur = next;
        }
        if chain.len() >= min_len {
            out.push(chain.into_iter().map(|i| [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5]).collect::<Vec<_>>());
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.len()));
    out
}

/// Whole pipeline: polylines in pixel coordinates, simplified.
pub fn trace(gray: &[u8], w: usize, h: usize, opts: TraceOptions) -> Vec<Vec<[f32; 2]>> {
    let e = edges(gray, w, h, opts);
    let min_len = (4.0 + 18.0 * (1.0 - opts.detail.clamp(0.0, 1.0))) as usize;
    chains(&e, w, h, min_len)
        .into_iter()
        .take(8000)
        .map(|c| {
            let pts: Vec<StrokePoint> = c.iter().map(|p| StrokePoint { x: p[0], y: p[1], pressure: 1.0 }).collect();
            simplify(&pts, 0.8).into_iter().map(|p| [p.x, p.y]).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark filled circle on white: one closed outline of the right size.
    #[test]
    fn traces_a_circle_outline() {
        let (w, h) = (200usize, 160usize);
        let gray: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                if ((x - 100.0).powi(2) + (y - 80.0).powi(2)).sqrt() < 50.0 { 30 } else { 230 }
            })
            .collect();
        let lines = trace(&gray, w, h, TraceOptions { detail: 0.5 });
        assert!(!lines.is_empty());
        let total: usize = lines.iter().map(|l| l.len()).sum();
        assert!(lines.len() <= 3, "one outline (maybe split), got {}", lines.len());
        // Every traced point lies on the circle's edge.
        for l in &lines {
            for p in l {
                let r = ((p[0] - 100.0).powi(2) + (p[1] - 80.0).powi(2)).sqrt();
                assert!((r - 50.0).abs() < 3.0, "point off the outline: r = {r}");
            }
        }
        // The outline goes all the way around (length ≈ 2π·50 ≈ 314 px).
        let length: f32 = lines.iter().map(|l| l.windows(2).map(|s| ((s[1][0] - s[0][0]).powi(2) + (s[1][1] - s[0][1]).powi(2)).sqrt()).sum::<f32>()).sum();
        assert!(length > 260.0, "outline length {length} ({total} points)");
        // Simplified: far fewer points than pixels.
        assert!(total < 150, "{total}");
    }

    #[test]
    fn flat_image_has_no_lines() {
        let gray = vec![128u8; 64 * 64];
        assert!(trace(&gray, 64, 64, TraceOptions { detail: 1.0 }).is_empty());
    }

    #[test]
    fn more_detail_more_lines() {
        // Noise-like texture plus a strong square: low detail keeps the square only.
        let (w, h) = (160usize, 160usize);
        let gray: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let base: u8 = if (40..120).contains(&x) && (40..120).contains(&y) { 40 } else { 220 };
                let texture = (((x * 7 + y * 13) % 17) as u8) * 3;
                base.saturating_add(texture)
            })
            .collect();
        let few = trace(&gray, w, h, TraceOptions { detail: 0.0 }).len();
        let many = trace(&gray, w, h, TraceOptions { detail: 1.0 }).len();
        assert!(few >= 1 && many >= few, "{few} vs {many}");
    }
}
