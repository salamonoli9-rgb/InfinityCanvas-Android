//! Video export of a flight through the bookmarks: frames are rendered by the app and fed to
//! a platform encoder (ffmpeg on desktop, MediaCodec on Android) on its own thread.

use canvas_core::camera::Camera;
use canvas_core::flight::{Bookmark, Flight};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// Receives RGBA frames (row-major, `w*h*4` bytes) and produces a video file.
pub trait VideoSink {
    fn push(&mut self, rgba: &[u8]) -> Result<(), String>;
    /// Finishes the file; returns where it went (for the user).
    fn finish(self: Box<Self>) -> Result<String, String>;
}

/// Creates a sink for (file name without extension, width, height, frames per second).
/// Called on the encoder thread (so JNI-based sinks attach that thread).
pub type VideoFactory = std::sync::Arc<dyn Fn(&str, u32, u32, u32) -> Result<Box<dyn VideoSink>, String> + Send + Sync>;

pub const FPS: u32 = 30;
/// Video width (height follows the screen's aspect): a multiple of 64 (GPU readback rows)
/// and of 16 (every encoder is happy).
pub const WIDTH: u32 = 768;
/// Pause at each bookmark, seconds.
const HOLD: f64 = 0.8;
/// Flights are a bit slower in the video than in the app (easier to follow).
const FLIGHT_SLOWDOWN: f64 = 1.3;

/// Cameras for every frame: hold at the first bookmark, then fly to each next one and hold.
/// `screen_vp` is the screen the bookmarks were made on; the cameras returned are for a video
/// of `size` showing the same area.
pub fn timeline(bookmarks: &[Bookmark], screen_vp: [f64; 2], size: [u32; 2]) -> Vec<Camera> {
    let k = size[0] as f64 / screen_vp[0];
    let vvp = [size[0] as f64, size[1] as f64];
    let cams: Vec<Camera> = bookmarks
        .iter()
        .map(|b| {
            let mut c = b.camera.clone();
            c.log_zoom += k.log2();
            c
        })
        .collect();
    let mut out = Vec::new();
    let hold = |c: &Camera, out: &mut Vec<Camera>| {
        for _ in 0..(HOLD * FPS as f64).round() as usize {
            out.push(c.clone());
        }
    };
    let Some(first) = cams.first() else { return out };
    hold(first, &mut out);
    for w in cams.windows(2) {
        let f = Flight::new(&w[0], &w[1], vvp);
        let frames = (f.duration * FLIGHT_SLOWDOWN * FPS as f64).round().max(2.0) as usize;
        for i in 1..=frames {
            out.push(f.at(f.duration * i as f64 / frames as f64));
        }
        hold(&w[1], &mut out);
    }
    out
}

/// Video size for a screen: [`WIDTH`] wide, same aspect, both sides multiples of 16.
pub fn size_for(screen_vp: [f64; 2]) -> [u32; 2] {
    let h = (WIDTH as f64 * screen_vp[1] / screen_vp[0] / 16.0).round().max(1.0) as u32 * 16;
    [WIDTH, h]
}

/// RGBA → NV12 (BT.601, limited range): the Y plane, then interleaved U/V at half resolution.
pub fn rgba_to_nv12(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3 / 2];
    let (y_plane, uv) = out.split_at_mut(w * h);
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate().take(w * h) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        y_plane[i] = (16.0 + (65.738 * r + 129.057 * g + 25.064 * b) / 256.0).round().clamp(0.0, 255.0) as u8;
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut acc = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y + dy).min(h - 1) * w + (x + dx).min(w - 1)) * 4;
                for c in 0..3 {
                    acc[c] += rgba[i + c] as f32 * 0.25;
                }
            }
            let (r, g, b) = (acc[0], acc[1], acc[2]);
            let u = 128.0 + (-37.945 * r - 74.494 * g + 112.439 * b) / 256.0;
            let v = 128.0 + (112.439 * r - 94.154 * g - 18.285 * b) / 256.0;
            let o = (y / 2) * w + x;
            uv[o] = u.round().clamp(0.0, 255.0) as u8;
            uv[o + 1] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// A running export: frames go to the encoder thread through a small queue.
pub struct VideoJob {
    tx: Option<SyncSender<Vec<u8>>>,
    result: Receiver<Result<String, String>>,
}

impl VideoJob {
    pub fn start(factory: VideoFactory, name: String, size: [u32; 2]) -> Self {
        let (tx, rx) = sync_channel::<Vec<u8>>(3);
        let (res_tx, result) = sync_channel(1);
        std::thread::spawn(move || {
            let r = (|| {
                let mut sink = factory(&name, size[0], size[1], FPS)?;
                for frame in rx {
                    sink.push(&frame)?;
                }
                sink.finish()
            })();
            let _ = res_tx.send(r);
        });
        Self { tx: Some(tx), result }
    }

    /// Queues a frame (waits if the encoder is behind). False if the encoder has failed.
    pub fn push(&mut self, rgba: Vec<u8>) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(rgba).is_ok())
    }

    /// No more frames; the result arrives through [`VideoJob::poll`].
    pub fn finish(&mut self) {
        self.tx = None;
    }

    pub fn poll(&self) -> Option<Result<String, String>> {
        self.result.try_recv().ok()
    }
}

/// Desktop: pipes raw frames into `ffmpeg` (H.264 MP4) in ~/Wideo or ~/Videos.
#[cfg(not(target_os = "android"))]
pub fn ffmpeg_factory() -> VideoFactory {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let base = ["Wideo", "Videos"].iter().map(|d| home.join(d)).find(|d| d.is_dir()).unwrap_or(home);
    ffmpeg_factory_in(base.join(crate::photos::EXPORT_FOLDER))
}

/// [`ffmpeg_factory`] writing into `dir`.
#[cfg(not(target_os = "android"))]
pub fn ffmpeg_factory_in(dir: std::path::PathBuf) -> VideoFactory {
    std::sync::Arc::new(move |name: &str, w: u32, h: u32, fps: u32| -> Result<Box<dyn VideoSink>, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{name}.mp4"));
        let child = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-movflags", "+faststart"])
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("ffmpeg niedostępny: {e}"))?;
        Ok(Box::new(FfmpegSink { child, path }))
    })
}

#[cfg(not(target_os = "android"))]
struct FfmpegSink {
    child: std::process::Child,
    path: std::path::PathBuf,
}

#[cfg(not(target_os = "android"))]
impl VideoSink for FfmpegSink {
    fn push(&mut self, rgba: &[u8]) -> Result<(), String> {
        use std::io::Write;
        self.child.stdin.as_mut().ok_or("ffmpeg closed")?.write_all(rgba).map_err(|e| e.to_string())
    }

    fn finish(mut self: Box<Self>) -> Result<String, String> {
        drop(self.child.stdin.take());
        let status = self.child.wait().map_err(|e| e.to_string())?;
        if status.success() { Ok(self.path.display().to_string()) } else { Err(format!("ffmpeg: {status}")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_of_flat_colours() {
        // White → Y 235, U = V = 128; pure red → Y 81, U 90, V 240 (BT.601 limited).
        let white = vec![255u8; 4 * 4 * 4];
        let n = rgba_to_nv12(&white, 4, 4);
        assert_eq!(n.len(), 24);
        assert!(n[..16].iter().all(|&y| y == 235) && n[16..].iter().all(|&c| c == 128));
        let red: Vec<u8> = (0..16).flat_map(|_| [255, 0, 0, 255]).collect();
        let n = rgba_to_nv12(&red, 4, 4);
        assert_eq!((n[0], n[16], n[17]), (81, 90, 240));
    }

    #[test]
    fn timeline_holds_and_flies() {
        let a = Bookmark { name: "a".into(), camera: Camera::default() };
        let mut far = Camera::default();
        far.pan_px([5000.0, 0.0]);
        let b = Bookmark { name: "b".into(), camera: far.clone() };
        let t = timeline(&[a, b], [1000.0, 2000.0], size_for([1000.0, 2000.0]));
        let hold = (HOLD * FPS as f64).round() as usize;
        assert!(t.len() > 2 * hold + 10);
        assert_eq!(t[0].center, t[hold - 1].center, "held at the start");
        assert_eq!(t.last().unwrap().center, far.center, "ends exactly at the last bookmark");
        assert_eq!(size_for([1080.0, 2400.0]), [768, 1712]);
    }
}
