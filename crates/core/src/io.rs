//! Binary file format: `b"ICNV"`, u16 LE version, postcard payload.

use crate::camera::Camera;
use crate::document::Document;
use crate::stroke::{Stroke, StrokeId};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAGIC: &[u8; 4] = b"ICNV";
pub const VERSION: u16 = 3;

/// Version 3: strokes carry flat caps where the eraser cut them (`Stroke::flat_caps`).
#[derive(Serialize, Deserialize)]
struct FileV3 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<Stroke>,
}

/// Version 2: strokes carry a draw order (`Stroke::order`), no caps.
#[derive(Deserialize)]
struct FileV2 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV2>,
}

#[derive(Deserialize)]
struct StrokeV2 {
    id: StrokeId,
    level: i32,
    anchor: [num_bigint::BigInt; 2],
    points: Vec<crate::stroke::StrokePoint>,
    color: [u8; 4],
    width: f32,
    order: u64,
}

impl From<StrokeV2> for Stroke {
    fn from(s: StrokeV2) -> Self {
        let mut out = Stroke::new(s.id, s.level, s.anchor, s.color, s.width);
        out.points = s.points;
        out.order = s.order;
        out
    }
}

/// Version 1: no draw order; the id was the order.
#[derive(Deserialize)]
struct FileV1 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV1>,
}

#[derive(Deserialize)]
struct StrokeV1 {
    id: StrokeId,
    level: i32,
    anchor: [num_bigint::BigInt; 2],
    points: Vec<crate::stroke::StrokePoint>,
    color: [u8; 4],
    width: f32,
}

impl From<StrokeV1> for Stroke {
    fn from(s: StrokeV1) -> Self {
        let mut out = Stroke::new(s.id, s.level, s.anchor, s.color, s.width);
        out.points = s.points;
        out
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    BadMagic,
    UnsupportedVersion(u16),
    Corrupt(postcard::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "I/O: {e}"),
            LoadError::BadMagic => write!(f, "not a canvas file"),
            LoadError::UnsupportedVersion(v) => write!(f, "unsupported file version {v}"),
            LoadError::Corrupt(e) => write!(f, "corrupt file: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

pub fn encode(doc: &Document, camera: &Camera) -> Vec<u8> {
    let file = FileV3 { camera: camera.clone(), next_id: doc.next_id(), strokes: doc.strokes().cloned().collect() };
    let mut out = Vec::with_capacity(64 + doc.len() * 128);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend(postcard::to_allocvec(&file).expect("serialization cannot fail"));
    out
}

pub fn decode(bytes: &[u8]) -> Result<(Document, Camera), LoadError> {
    if bytes.len() < 6 || &bytes[..4] != MAGIC {
        return Err(LoadError::BadMagic);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    match version {
        1 => {
            let f: FileV1 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera))
        }
        2 => {
            let f: FileV2 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera))
        }
        3 => {
            let f: FileV3 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes, f.next_id), f.camera))
        }
        v => Err(LoadError::UnsupportedVersion(v)),
    }
}

/// Writes atomically: temp file + fsync + rename, so a crash never leaves a broken file.
pub fn save_file(path: &Path, doc: &Document, camera: &Camera) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&encode(doc, camera))?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn load_file(path: &Path) -> Result<(Document, Camera), LoadError> {
    decode(&std::fs::read(path).map_err(LoadError::Io)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::StrokePoint;
    use num_bigint::BigInt;

    #[test]
    fn roundtrip_with_huge_coordinates() {
        let mut doc = Document::new();
        let mut cam = Camera::default();
        cam.zoom_at([1.0, 2.0], [100.0, 100.0], 333.25);
        let anchor = [BigInt::from(-7) << 400u32, BigInt::from(12345) << 333u32];
        let mut s = Stroke::new(doc.alloc_id(), 333, anchor, [1, 2, 3, 255], 3.5);
        s.push(StrokePoint { x: 1.0, y: -2.0, pressure: 0.5 });
        s.push(StrokePoint { x: 5.0, y: 2.0, pressure: 0.7 });
        doc.insert(s.clone());
        let bytes = encode(&doc, &cam);
        let (d2, c2) = decode(&bytes).unwrap();
        assert_eq!(c2, cam);
        assert_eq!(d2.get(s.id), Some(&s));
        assert_eq!(d2.next_id(), doc.next_id());

        let dir = std::env::temp_dir().join(format!("icnv-test-{}", std::process::id()));
        let path = dir.join("a.icnv");
        save_file(&path, &doc, &cam).unwrap();
        assert_eq!(load_file(&path).unwrap().0.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(decode(b"nope"), Err(LoadError::BadMagic)));
        assert!(matches!(decode(b"ICNV\x09\x00"), Err(LoadError::UnsupportedVersion(9))));
        assert!(matches!(decode(b"ICNV\x03\x00\xff"), Err(LoadError::Corrupt(_))));
    }

    #[test]
    fn reads_version_1() {
        #[derive(Serialize)]
        struct S1 {
            id: StrokeId,
            level: i32,
            anchor: [BigInt; 2],
            points: Vec<StrokePoint>,
            color: [u8; 4],
            width: f32,
        }
        #[derive(Serialize)]
        struct F1 {
            camera: Camera,
            next_id: StrokeId,
            strokes: Vec<S1>,
        }
        let pts = vec![StrokePoint { x: 1.0, y: 2.0, pressure: 1.0 }, StrokePoint { x: 3.0, y: 2.0, pressure: 1.0 }];
        let f = F1 {
            camera: Camera::default(),
            next_id: 8,
            strokes: vec![S1 { id: 7, level: -3, anchor: [BigInt::from(5), BigInt::from(-5)], points: pts.clone(), color: [9, 9, 9, 255], width: 2.0 }],
        };
        let mut bytes = b"ICNV\x01\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, _) = decode(&bytes).unwrap();
        let s = d.get(7).unwrap();
        assert_eq!((s.order, s.level, &s.points, s.width), (7, -3, &pts, 2.0));
        assert_eq!(d.next_id(), 8);
    }

    #[test]
    fn reads_version_2() {
        #[derive(Serialize)]
        struct S2 {
            id: StrokeId,
            level: i32,
            anchor: [BigInt; 2],
            points: Vec<StrokePoint>,
            color: [u8; 4],
            width: f32,
            order: u64,
        }
        #[derive(Serialize)]
        struct F2 {
            camera: Camera,
            next_id: StrokeId,
            strokes: Vec<S2>,
        }
        let pts = vec![StrokePoint { x: 1.0, y: 2.0, pressure: 1.0 }, StrokePoint { x: 3.0, y: 2.0, pressure: 1.0 }];
        let f = F2 {
            camera: Camera::default(),
            next_id: 10,
            strokes: vec![S2 { id: 9, level: 4, anchor: [BigInt::from(1), BigInt::from(2)], points: pts.clone(), color: [1, 2, 3, 255], width: 5.0, order: 3 }],
        };
        let mut bytes = b"ICNV\x02\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, _) = decode(&bytes).unwrap();
        let s = d.get(9).unwrap();
        assert_eq!((s.order, s.flat_caps, &s.points), (3, [false, false], &pts));
    }
}
