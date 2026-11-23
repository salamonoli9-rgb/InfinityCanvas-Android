//! Photo import: decoding (with EXIF orientation), resizing and re-encoding for storage.

use image::{DynamicImage, ImageDecoder, RgbaImage};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Longest side of a stored reference photo, px.
pub const MAX_SIDE: u32 = 2048;

/// Decodes JPEG / PNG / WebP and applies the EXIF orientation.
pub fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Some(img)
}

/// A photo ready to be placed: pixels for display and a compact encoding for the file.
pub struct Prepared {
    pub rgba: RgbaImage,
    pub encoded: Vec<u8>,
}

/// Decodes, scales down to [`MAX_SIDE`] and re-encodes as JPEG (PNG if it has transparency).
pub fn prepare(bytes: &[u8]) -> Option<Prepared> {
    let img = decode(bytes)?;
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let has_alpha = rgba.pixels().any(|p| p.0[3] < 255);
    let mut encoded = Vec::new();
    if has_alpha {
        rgba.write_to(&mut Cursor::new(&mut encoded), image::ImageFormat::Png).ok()?;
    } else {
        let rgb = DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 88);
        rgb.write_with_encoder(enc).ok()?;
    }
    Some(Prepared { rgba, encoded })
}

/// Small preview for pickers.
pub fn thumbnail(bytes: &[u8], side: u32) -> Option<RgbaImage> {
    Some(decode(bytes)?.thumbnail(side, side).to_rgba8())
}

/// One photo the user can pick.
#[derive(Clone, Debug)]
pub struct PhotoEntry {
    /// Platform key (file path, content URI…).
    pub key: String,
    pub name: String,
}

/// Where photos come from (desktop folders, Android MediaStore). Runs on a worker thread.
pub trait PhotoBackend: Send {
    /// Most recent first. `Err` carries a message for the user (e.g. permission needed).
    fn list(&mut self) -> Result<Vec<PhotoEntry>, String>;
    fn read(&mut self, key: &str) -> Option<Vec<u8>>;
    fn thumbnail(&mut self, key: &str, side: u32) -> Option<RgbaImage> {
        thumbnail(&self.read(key)?, side)
    }
    /// Asks the system for access to photos (Android permission dialog).
    fn request_access(&mut self) {}
}

enum Request {
    List,
    Thumb(String),
    Read(String),
    Access,
}

enum Response {
    List(Result<Vec<PhotoEntry>, String>),
    Thumb(String, Option<RgbaImage>),
    Read(String, Option<Vec<u8>>),
}

/// Asynchronous front of a [`PhotoBackend`]: requests go to a worker thread, results are
/// collected with [`PhotoLibrary::poll`] once per frame.
pub struct PhotoLibrary {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    pub entries: Option<Result<Vec<PhotoEntry>, String>>,
    listing: bool,
    pub thumbs: HashMap<String, Option<RgbaImage>>,
    requested: HashSet<String>,
    /// Bytes of a photo the user picked, once loaded.
    pub loaded: Option<(String, Option<Vec<u8>>)>,
    pub busy: bool,
}

pub const THUMB_SIDE: u32 = 200;

impl PhotoLibrary {
    pub fn new(mut backend: Box<dyn PhotoBackend>) -> Self {
        let (tx, worker_rx) = channel::<Request>();
        let (worker_tx, rx) = channel::<Response>();
        std::thread::spawn(move || {
            for req in worker_rx {
                let resp = match req {
                    Request::List => Response::List(backend.list()),
                    Request::Thumb(k) => {
                        let t = backend.thumbnail(&k, THUMB_SIDE);
                        Response::Thumb(k, t)
                    }
                    Request::Read(k) => {
                        let b = backend.read(&k);
                        Response::Read(k, b)
                    }
                    Request::Access => {
                        backend.request_access();
                        continue;
                    }
                };
                if worker_tx.send(resp).is_err() {
                    break;
                }
            }
        });
        Self { tx, rx, entries: None, listing: false, thumbs: HashMap::new(), requested: HashSet::new(), loaded: None, busy: false }
    }

    /// (Re)lists the photos.
    pub fn refresh(&mut self) {
        if !self.listing {
            self.listing = true;
            let _ = self.tx.send(Request::List);
        }
    }

    pub fn request_thumb(&mut self, key: &str) {
        if self.requested.insert(key.to_string()) {
            let _ = self.tx.send(Request::Thumb(key.to_string()));
        }
    }

    pub fn load(&mut self, key: &str) {
        self.busy = true;
        let _ = self.tx.send(Request::Read(key.to_string()));
    }

    pub fn request_access(&mut self) {
        let _ = self.tx.send(Request::Access);
    }

    /// Collects finished work. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(r) = self.rx.try_recv() {
            changed = true;
            match r {
                Response::List(l) => {
                    self.listing = false;
                    self.entries = Some(l);
                }
                Response::Thumb(k, t) => {
                    self.thumbs.insert(k, t);
                }
                Response::Read(k, b) => {
                    self.busy = false;
                    self.loaded = Some((k, b));
                }
            }
        }
        changed || self.listing || self.busy || self.requested.len() > self.thumbs.len()
    }
}

/// Desktop: image files in the usual picture folders (and their direct subfolders).
pub struct FolderPhotos {
    pub dirs: Vec<std::path::PathBuf>,
}

impl FolderPhotos {
    pub fn default_dirs() -> Self {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        let dirs = ["Obrazy", "Pictures", "Pobrane", "Downloads", "Zdjęcia", "Desktop", "Pulpit"]
            .iter()
            .map(|d| home.join(d))
            .filter(|d| d.is_dir())
            .collect();
        Self { dirs }
    }
}

impl PhotoBackend for FolderPhotos {
    fn list(&mut self) -> Result<Vec<PhotoEntry>, String> {
        let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
        fn walk(dir: &std::path::Path, depth: u32, found: &mut Vec<(std::time::SystemTime, std::path::PathBuf)>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if depth > 0 {
                        walk(&p, depth - 1, found);
                    }
                } else if let Some(ext) = p.extension().and_then(|x| x.to_str())
                    && ["jpg", "jpeg", "png", "webp"].contains(&ext.to_ascii_lowercase().as_str())
                {
                    let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                    found.push((t, p));
                }
            }
        }
        for d in &self.dirs {
            walk(d, 1, &mut found);
        }
        if found.is_empty() {
            return Err("Brak zdjęć w folderach Obrazy / Pictures / Pobrane. Możesz też przeciągnąć plik na okno.".into());
        }
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.truncate(300);
        Ok(found
            .into_iter()
            .map(|(_, p)| PhotoEntry {
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                key: p.to_string_lossy().into_owned(),
            })
            .collect())
    }

    fn read(&mut self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(key).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_scales_and_roundtrips() {
        let mut img = RgbaImage::new(3000, 1000);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, 100, 200, 255]);
        }
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = prepare(&png).unwrap();
        assert_eq!(p.rgba.dimensions(), (2048, 683));
        assert_eq!(&p.encoded[..2], &[0xff, 0xd8], "opaque photos are stored as JPEG");
        let back = decode(&p.encoded).unwrap();
        assert_eq!((back.width(), back.height()), (2048, 683));
        assert!(decode(b"not an image").is_none());
    }
}
