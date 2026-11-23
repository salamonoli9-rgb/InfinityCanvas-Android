//! Reference photos placed on the canvas (e.g. to trace over). They live in canvas
//! coordinates like strokes: an exact anchor at a scale level plus an f64 offset, size and
//! rotation in that level's units. They are raster images, so they do blur when magnified a lot.

use crate::camera::{Camera, LevelFrame, rotate};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type ImageId = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefImage {
    pub id: ImageId,
    pub level: i32,
    /// Integer anchor in level units.
    pub anchor: [BigInt; 2],
    /// Top-left corner relative to the anchor, level units.
    pub pos: [f64; 2],
    /// Width and height in level units.
    pub size: [f64; 2],
    /// Rotation around the top-left corner, radians.
    pub rotation: f64,
    /// 0..1
    pub opacity: f32,
    pub visible: bool,
    /// Pixel size of the encoded image.
    pub pixels: [u32; 2],
    /// Encoded image (JPEG or PNG). Shared, so undo history doesn't copy it.
    pub data: Arc<Vec<u8>>,
}

impl RefImage {
    /// Places an image centred on the screen, fitting `fit` (0..1) of the viewport and
    /// upright on screen.
    pub fn place(id: ImageId, cam: &Camera, viewport: [f64; 2], pixels: [u32; 2], data: Arc<Vec<u8>>, fit: f64) -> Self {
        let level = cam.level();
        let c = cam.screen_to_world([viewport[0] * 0.5, viewport[1] * 0.5], viewport).mul_pow2(level);
        let anchor = [c.x.floor_at(0), c.y.floor_at(0)];
        let f = cam.level_frame(level, viewport);
        let k = (viewport[0] * fit / pixels[0].max(1) as f64).min(viewport[1] * fit / pixels[1].max(1) as f64);
        let size = [pixels[0] as f64 * k / f.scale, pixels[1] as f64 * k / f.scale];
        let rotation = -cam.rotation;
        let centre = f.screen_to_local([viewport[0] * 0.5, viewport[1] * 0.5], &anchor, None);
        let half = rotate([size[0] * 0.5, size[1] * 0.5], [rotation.cos(), rotation.sin()]);
        let pos = [centre[0] - half[0], centre[1] - half[1]];
        Self { id, level, anchor, pos, size, rotation, opacity: 0.5, visible: true, pixels, data }
    }

    fn rot(&self) -> [f64; 2] {
        [self.rotation.cos(), self.rotation.sin()]
    }

    fn anchor_fast(&self) -> Option<[i128; 2]> {
        self.anchor[0].to_i128().zip(self.anchor[1].to_i128()).map(|(a, b)| [a, b])
    }

    /// Point of the image at texture coordinates (u, v) ∈ [0, 1]², in local units.
    pub fn local_at(&self, u: f64, v: f64) -> [f64; 2] {
        let d = rotate([u * self.size[0], v * self.size[1]], self.rot());
        [self.pos[0] + d[0], self.pos[1] + d[1]]
    }

    /// Screen positions of the corners (0,0), (1,0), (1,1), (0,1).
    pub fn screen_corners(&self, cam: &Camera, viewport: [f64; 2]) -> [[f64; 2]; 4] {
        let f = cam.level_frame(self.level, viewport);
        let o = f.anchor_to_screen(&self.anchor, self.anchor_fast());
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
            let s = f.local_vec_to_screen(self.local_at(u, v));
            [o[0] + s[0], o[1] + s[1]]
        })
    }

    fn frame(&self, cam: &Camera, viewport: [f64; 2]) -> LevelFrame {
        cam.level_frame(self.level, viewport)
    }

    /// Moves the image by `d` screen pixels.
    pub fn pan(&mut self, cam: &Camera, viewport: [f64; 2], d: [f64; 2]) {
        let v = self.frame(cam, viewport).screen_vec_to_local(d);
        self.pos = [self.pos[0] + v[0], self.pos[1] + v[1]];
    }

    /// Scales by `2^dlog` keeping screen point `c` fixed.
    pub fn zoom_at(&mut self, cam: &Camera, viewport: [f64; 2], c: [f64; 2], dlog: f64) {
        let k = dlog.exp2();
        if !k.is_finite() {
            return;
        }
        let pc = self.frame(cam, viewport).screen_to_local(c, &self.anchor, self.anchor_fast());
        self.pos = [pc[0] + (self.pos[0] - pc[0]) * k, pc[1] + (self.pos[1] - pc[1]) * k];
        self.size = [self.size[0] * k, self.size[1] * k];
    }

    /// Rotates by `angle` keeping screen point `c` fixed.
    pub fn rotate_at(&mut self, cam: &Camera, viewport: [f64; 2], c: [f64; 2], angle: f64) {
        let pc = self.frame(cam, viewport).screen_to_local(c, &self.anchor, self.anchor_fast());
        let d = rotate([self.pos[0] - pc[0], self.pos[1] - pc[1]], [angle.cos(), angle.sin()]);
        self.pos = [pc[0] + d[0], pc[1] + d[1]];
        self.rotation = crate::camera::wrap_angle(self.rotation + angle);
    }

    /// Image coordinates (u, v) of a screen point; inside the image when both are in [0, 1].
    pub fn uv_at(&self, cam: &Camera, viewport: [f64; 2], p: [f64; 2]) -> [f64; 2] {
        let l = self.frame(cam, viewport).screen_to_local(p, &self.anchor, self.anchor_fast());
        let d = crate::camera::unrotate([l[0] - self.pos[0], l[1] - self.pos[1]], self.rot());
        [d[0] / self.size[0], d[1] / self.size[1]]
    }

    pub fn contains(&self, cam: &Camera, viewport: [f64; 2], p: [f64; 2]) -> bool {
        let uv = self.uv_at(cam, viewport, p);
        (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: [f64; 2] = [1000.0, 800.0];

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn placed_centred_and_upright_even_deep_and_rotated() {
        let mut cam = Camera::default();
        cam.zoom_at([10.0, 20.0], VP, 200.5);
        cam.rotate_at([300.0, 300.0], VP, 0.6);
        let img = RefImage::place(1, &cam, VP, [400, 200], Arc::new(vec![]), 0.5);
        let c = img.screen_corners(&cam, VP);
        // 400×200 px fitted into half of 1000×800 → 500×250 on screen, centred, axis-aligned.
        assert!(close(c[0], [250.0, 275.0]), "{:?}", c[0]);
        assert!(close(c[2], [750.0, 525.0]), "{:?}", c[2]);
        assert!(close(c[1], [750.0, 275.0]));
        assert!(img.contains(&cam, VP, [500.0, 400.0]) && !img.contains(&cam, VP, [200.0, 400.0]));
    }

    #[test]
    fn pan_zoom_rotate_follow_the_fingers() {
        let cam = Camera::default();
        let mut img = RefImage::place(1, &cam, VP, [100, 100], Arc::new(vec![]), 0.5);
        let before = img.screen_corners(&cam, VP);
        img.pan(&cam, VP, [10.0, -5.0]);
        let after = img.screen_corners(&cam, VP);
        assert!(close(after[0], [before[0][0] + 10.0, before[0][1] - 5.0]));
        // Zoom ×2 around the top-left corner: it stays, the opposite corner moves away.
        let tl = after[0];
        img.zoom_at(&cam, VP, tl, 1.0);
        let z = img.screen_corners(&cam, VP);
        assert!(close(z[0], tl));
        assert!(close(z[2], [tl[0] + 2.0 * (after[2][0] - tl[0]), tl[1] + 2.0 * (after[2][1] - tl[1])]));
        // Quarter turn around the top-left corner.
        img.rotate_at(&cam, VP, tl, std::f64::consts::FRAC_PI_2);
        let r = img.screen_corners(&cam, VP);
        assert!(close(r[0], tl));
        let w = z[1][0] - z[0][0];
        assert!(close(r[1], [tl[0], tl[1] + w]), "{:?}", r[1]);
    }
}
