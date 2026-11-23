//! Photo → coloured illustration: colours reduced to a small palette (k-means), speckles
//! merged into their neighbours, and every colour patch traced into a polygon.
//!
//! Patches come back sorted back to front (largest enclosed area first), so painting them in
//! order reproduces the picture: a patch inside another one is always drawn after it.

#[derive(Clone, Debug)]
pub struct Patch {
    pub color: [u8; 3],
    /// Outer boundary in pixel coordinates (corners of pixels), simplified.
    pub polygon: Vec<[f32; 2]>,
    /// Area enclosed by the polygon, px².
    pub area: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PosterizeOptions {
    /// 0 = few colours and big patches, 1 = many colours and small patches.
    pub detail: f32,
}

fn blur_rgb(rgb: &[u8], w: usize, h: usize) -> Vec<[f32; 3]> {
    // 3×3 box blur twice ≈ small Gaussian: removes JPEG noise before clustering.
    let mut img: Vec<[f32; 3]> = rgb.as_chunks::<3>().0.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    for _ in 0..2 {
        let src = img.clone();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                let mut n = 0.0;
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (sx, sy) = (x as isize + dx, y as isize + dy);
                        if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                            let p = src[sy as usize * w + sx as usize];
                            for c in 0..3 {
                                acc[c] += p[c];
                            }
                            n += 1.0;
                        }
                    }
                }
                img[y * w + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        }
    }
    img
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    // Rough perceptual weights (green matters most, blue least).
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    2.0 * d[0] * d[0] + 4.0 * d[1] * d[1] + 3.0 * d[2] * d[2]
}

/// k-means palette (farthest-point initialisation, deterministic).
fn palette(px: &[[f32; 3]], k: usize) -> Vec<[f32; 3]> {
    let sample: Vec<[f32; 3]> = px.iter().step_by((px.len() / 8000).max(1)).copied().collect();
    let mean = sample.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let n = sample.len() as f32;
    let mut centres = vec![[mean[0] / n, mean[1] / n, mean[2] / n]];
    let mut nearest: Vec<f32> = sample.iter().map(|p| dist2(*p, centres[0])).collect();
    while centres.len() < k {
        let (i, d) = nearest.iter().enumerate().fold((0, -1.0), |best, (i, &d)| if d > best.1 { (i, d) } else { best });
        if d <= 1.0 {
            break; // fewer distinct colours than k
        }
        let c = sample[i];
        centres.push(c);
        for (j, p) in sample.iter().enumerate() {
            nearest[j] = nearest[j].min(dist2(*p, c));
        }
    }
    for round in 0..14 {
        let mut sum = vec![[0.0f32; 4]; centres.len()];
        for p in &sample {
            let i = closest(&centres, *p);
            sum[i] = [sum[i][0] + p[0], sum[i][1] + p[1], sum[i][2] + p[2], sum[i][3] + 1.0];
        }
        for (c, s) in centres.iter_mut().zip(&sum) {
            if s[3] > 0.0 {
                *c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
        // Drop colours almost nobody uses (blends along edges, noise): their pixels go to
        // the nearest real colour instead of forming slivers.
        if round == 8 {
            let min = n * 0.003;
            let kept: Vec<[f32; 3]> = centres.iter().zip(&sum).filter(|(_, s)| s[3] >= min).map(|(c, _)| *c).collect();
            if !kept.is_empty() {
                centres = kept;
            }
        }
    }
    centres
}

fn closest(centres: &[[f32; 3]], p: [f32; 3]) -> usize {
    centres
        .iter()
        .enumerate()
        .map(|(i, c)| (i, dist2(*c, p)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Connected components (4-neighbourhood) of equal labels: component id per pixel and sizes.
fn components(labels: &[u16], w: usize, h: usize) -> (Vec<u32>, Vec<u32>) {
    let mut comp = vec![u32::MAX; w * h];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if comp[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let l = labels[start];
        let mut size = 0;
        comp[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if comp[j] == u32::MAX && labels[j] == l {
                    comp[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        sizes.push(size);
    }
    (comp, sizes)
}

/// Recolours components smaller than `min_area` with their most common neighbouring label.
fn merge_small(labels: &mut [u16], w: usize, h: usize, min_area: u32) {
    for _ in 0..8 {
        let (comp, sizes) = components(labels, w, h);
        let small: Vec<bool> = sizes.iter().map(|&s| s < min_area).collect();
        if !small.iter().any(|&s| s) {
            return;
        }
        // Votes: for each small component, labels of the pixels around it.
        let mut votes: std::collections::HashMap<u32, std::collections::HashMap<u16, u32>> = Default::default();
        for i in 0..w * h {
            let c = comp[i];
            if !small[c as usize] {
                continue;
            }
            let (x, y) = (i % w, i / w);
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h {
                    let j = ny * w + nx;
                    if comp[j] != c {
                        // Big neighbours count much more: speckle rings shouldn't vote each
                        // other into place.
                        let weight = if small[comp[j] as usize] { 1 } else { 16 };
                        *votes.entry(c).or_default().entry(labels[j]).or_default() += weight;
                    }
                }
            }
        }
        let target: std::collections::HashMap<u32, u16> =
            votes.into_iter().filter_map(|(c, v)| v.into_iter().max_by_key(|&(_, n)| n).map(|(l, _)| (c, l))).collect();
        let mut changed = false;
        for i in 0..w * h {
            if let Some(&l) = target.get(&comp[i]) {
                labels[i] = l;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

/// Outer boundary of the component containing `start` (its first pixel in scan order),
/// following pixel edges clockwise (region on the right). Returns corner points at turns.
fn trace_boundary(comp: &[u32], w: usize, h: usize, start: usize) -> Vec<[f32; 2]> {
    let id = comp[start];
    let inside = |x: isize, y: isize| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && comp[y as usize * w + x as usize] == id;
    // Directions: east, south, west, north (y down).
    const D: [(isize, isize); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let (x0, y0) = ((start % w) as isize, (start / w) as isize);
    let (mut x, mut y, mut d) = (x0, y0, 0usize);
    let mut out = vec![[x0 as f32, y0 as f32]];
    let limit = 4 * (w + 1) * (h + 1);
    for _ in 0..limit {
        // Move one edge along d.
        x += D[d].0;
        y += D[d].1;
        if x == x0 && y == y0 && d == 3 {
            break;
        }
        // Pixels ahead-left and ahead-right of the corner (x, y) for direction d. The pixel
        // whose top-left corner is c and lies toward (dx, dy) is at c + min(0, dx), min(0, dy)…
        let (dx, dy) = D[d];
        let (rx, ry) = (-dy, dx); // right of travel
        let pix = |ax: isize, ay: isize| {
            // Pixel touching corner (x, y) in quadrant (ax, ay) ∈ {-1, 1}².
            inside(x + if ax < 0 { -1 } else { 0 }, y + if ay < 0 { -1 } else { 0 })
        };
        let ahead_left = pix(dx - rx, dy - ry);
        let ahead_right = pix(dx + rx, dy + ry);
        let nd = if ahead_left {
            (d + 3) % 4
        } else if ahead_right {
            d
        } else {
            (d + 1) % 4
        };
        if nd != d {
            out.push([x as f32, y as f32]);
        }
        d = nd;
        if x == x0 && y == y0 && d == 0 {
            break;
        }
    }
    out
}

/// Bucket fill: the region of non-ink pixels connected to `seed` (4-neighbourhood), as an
/// outer outline plus the outlines of its holes (islands of ink or other enclosed areas).
/// `None` if the seed is on ink or the region leaks out to the image border (not enclosed).
pub fn fill_region(ink: &[bool], w: usize, h: usize, seed: (usize, usize)) -> Option<Vec<Vec<[f32; 2]>>> {
    let s = seed.1 * w + seed.0;
    if seed.0 >= w || seed.1 >= h || ink[s] {
        return None;
    }
    // Flood the region.
    let mut region = vec![false; w * h];
    let mut stack = vec![s];
    region[s] = true;
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
            return None; // open to the outside
        }
        for j in [i - 1, i + 1, i - w, i + w] {
            if !region[j] && !ink[j] {
                region[j] = true;
                stack.push(j);
            }
        }
    }
    let ids: Vec<u32> = region.iter().map(|&r| u32::from(r)).collect();
    let first = region.iter().position(|&r| r)?;
    let mut loops = vec![trace_boundary(&ids, w, h, first)];
    // Holes: components of the rest that don't touch the border (they are inside the region).
    let rest: Vec<u16> = region.iter().map(|&r| u16::from(!r)).collect();
    let (comp, sizes) = components(&rest, w, h);
    let mut touches_border = vec![false; sizes.len()];
    for x in 0..w {
        touches_border[comp[x] as usize] = true;
        touches_border[comp[(h - 1) * w + x] as usize] = true;
    }
    for y in 0..h {
        touches_border[comp[y * w] as usize] = true;
        touches_border[comp[y * w + w - 1] as usize] = true;
    }
    let mut seen = vec![false; sizes.len()];
    for i in 0..w * h {
        let c = comp[i] as usize;
        if rest[i] == 0 || seen[c] || touches_border[c] {
            continue;
        }
        seen[c] = true;
        if sizes[c] >= 3 {
            loops.push(trace_boundary(&comp, w, h, i));
        }
    }
    Some(
        loops
            .into_iter()
            .map(|l| {
                let pts: Vec<crate::stroke::StrokePoint> = l.iter().map(|p| crate::stroke::StrokePoint { x: p[0], y: p[1], pressure: 1.0 }).collect();
                crate::mesh::simplify(&pts, 0.6).iter().map(|p| [p.x, p.y]).collect::<Vec<_>>()
            })
            .filter(|l: &Vec<[f32; 2]>| l.len() >= 3)
            .collect(),
    )
}

fn polygon_area(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    0.5 * (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f32>().abs()
}

/// The whole pipeline. `rgb` is row-major RGB, 3 bytes per pixel.
pub fn posterize(rgb: &[u8], w: usize, h: usize, opts: PosterizeOptions) -> Vec<Patch> {
    assert_eq!(rgb.len(), w * h * 3);
    let detail = opts.detail.clamp(0.0, 1.0);
    let px = blur_rgb(rgb, w, h);
    let k = (6.0 + 14.0 * detail).round() as usize;
    let centres = palette(&px, k);
    let mut labels: Vec<u16> = px.iter().map(|p| closest(&centres, *p) as u16).collect();
    let min_area = ((w * h) as f32 * (0.004 - 0.0035 * detail)).max(8.0) as u32;
    merge_small(&mut labels, w, h, min_area);
    let (comp, sizes) = components(&labels, w, h);
    let mut seen = vec![false; sizes.len()];
    let mut patches = Vec::new();
    for i in 0..w * h {
        let c = comp[i] as usize;
        if seen[c] {
            continue;
        }
        seen[c] = true;
        let boundary = trace_boundary(&comp, w, h, i);
        if boundary.len() < 3 {
            continue;
        }
        let pts: Vec<crate::stroke::StrokePoint> =
            boundary.iter().map(|p| crate::stroke::StrokePoint { x: p[0], y: p[1], pressure: 1.0 }).collect();
        let mut simple: Vec<[f32; 2]> = crate::mesh::simplify(&pts, 0.7).iter().map(|p| [p.x, p.y]).collect();
        if simple.len() < 3 {
            simple = boundary;
        }
        let area = polygon_area(&simple);
        let c = centres[labels[i] as usize];
        patches.push(Patch { color: [c[0] as u8, c[1] as u8, c[2] as u8], polygon: simple, area });
    }
    patches.sort_by(|a, b| b.area.total_cmp(&a.area));
    patches.truncate(3000);
    patches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|i| f(i % w, i / w)).collect()
    }

    #[test]
    fn boundary_of_a_rectangle() {
        // Component: pixels x 2..6, y 1..4 → corners (2,1) (6,1) (6,4) (2,4).
        let (w, h) = (8, 6);
        let comp: Vec<u32> = (0..w * h).map(|i| if (2..6).contains(&(i % w)) && (1..4).contains(&(i / w)) { 1 } else { 0 }).collect();
        let b = trace_boundary(&comp, w, h, w + 2);
        assert_eq!(b, vec![[2.0, 1.0], [6.0, 1.0], [6.0, 4.0], [2.0, 4.0]]);
        assert_eq!(polygon_area(&b), 12.0);
    }

    #[test]
    fn two_colour_picture_gives_matching_patches() {
        // Red background with a blue disc: a big red patch first, then the blue one on top.
        let (w, h) = (120, 100);
        let rgb = image(w, h, |x, y| {
            if ((x as f32 - 60.0).powi(2) + (y as f32 - 50.0).powi(2)).sqrt() < 25.0 { [20, 40, 220] } else { [230, 30, 40] }
        });
        let p = posterize(&rgb, w, h, PosterizeOptions { detail: 0.3 });
        assert_eq!(p.len(), 2, "{:?}", p.iter().map(|q| (q.color, q.area)).collect::<Vec<_>>());
        assert!(p[0].color[0] > 180 && p[0].area > 11_000.0, "background first: {:?}", p[0].color);
        assert!(p[1].color[2] > 180, "disc on top: {:?}", p[1].color);
        let expected = std::f32::consts::PI * 25.0 * 25.0;
        assert!((p[1].area - expected).abs() < expected * 0.12, "{} vs {expected}", p[1].area);
    }

    #[test]
    fn ring_is_drawn_before_its_contents() {
        // Green ring with a yellow disc inside it, on white: the ring's outer polygon encloses
        // the disc, so it must come first even though it has fewer pixels.
        let (w, h) = (100, 100);
        let rgb = image(w, h, |x, y| {
            let r = ((x as f32 - 50.0).powi(2) + (y as f32 - 50.0).powi(2)).sqrt();
            if r < 30.0 {
                [240, 220, 30]
            } else if r < 36.0 {
                [30, 160, 60]
            } else {
                [250, 250, 250]
            }
        });
        let p = posterize(&rgb, w, h, PosterizeOptions { detail: 0.3 });
        let ring = p.iter().position(|q| q.color[1] > 120 && q.color[0] < 100).expect("ring");
        let disc = p.iter().position(|q| q.color[0] > 200 && q.color[2] < 100).expect("disc");
        assert!(ring < disc, "ring {ring} must be painted before disc {disc}");
    }

    #[test]
    fn speckles_are_merged() {
        // Grey with salt-and-pepper noise: one patch, not hundreds.
        let (w, h) = (80, 80);
        let rgb = image(w, h, |x, y| if (x * 31 + y * 17) % 97 == 0 { [0, 0, 0] } else { [128, 128, 128] });
        let p = posterize(&rgb, w, h, PosterizeOptions { detail: 0.5 });
        assert!(p.len() <= 2, "{}", p.len());
    }

    #[test]
    fn bucket_fill_inside_a_ring_with_a_hole() {
        // Ink: a square ring (outline) and a small filled square inside it (an "island").
        let (w, h) = (60, 60);
        let ink: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let ring = (10..50).contains(&x) && (10..50).contains(&y) && !((12..48).contains(&x) && (12..48).contains(&y));
                let island = (25..35).contains(&x) && (25..35).contains(&y);
                ring || island
            })
            .collect();
        let loops = fill_region(&ink, w, h, (15, 15)).expect("enclosed");
        assert_eq!(loops.len(), 2, "outline + hole");
        let a0 = polygon_area(&loops[0]);
        let a1 = polygon_area(&loops[1]);
        assert!((a0 - 36.0 * 36.0).abs() < 1.0, "inside of the ring: {a0}");
        assert!((a1 - 100.0).abs() < 1.0, "island: {a1}");
        // Outside the ring is open to the border; on ink there is nothing to fill.
        assert!(fill_region(&ink, w, h, (5, 5)).is_none());
        assert!(fill_region(&ink, w, h, (10, 30)).is_none());
    }
}
