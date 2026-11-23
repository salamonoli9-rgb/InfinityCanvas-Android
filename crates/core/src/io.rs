//! Binary file format: `b"ICNV"`, u16 LE version, postcard payload.

use crate::camera::Camera;
use crate::document::Document;
use crate::stroke::{Stroke, StrokeId};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAGIC: &[u8; 4] = b"ICNV";
pub const VERSION: u16 = 8;

/// Version 8: layers (strokes carry a layer id).
#[derive(Serialize, Deserialize)]
struct FileV8 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<Stroke>,
    images: Vec<crate::image::RefImage>,
    bookmarks: Vec<crate::flight::Bookmark>,
    layers: Vec<crate::document::Layer>,
}

/// Version 7: bookmarks (saved views).
#[derive(Deserialize)]
struct FileV7 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV5>,
    images: Vec<crate::image::RefImage>,
    bookmarks: Vec<crate::flight::Bookmark>,
}

/// Stroke of versions 5–7 (no layer).
#[derive(Deserialize)]
struct StrokeV5 {
    id: StrokeId,
    level: i32,
    anchor: [num_bigint::BigInt; 2],
    points: Vec<crate::stroke::StrokePoint>,
    color: [u8; 4],
    width: f32,
    order: u64,
    flat_caps: [bool; 2],
    brush: crate::stroke::Brush,
}

impl From<StrokeV5> for Stroke {
    fn from(s: StrokeV5) -> Self {
        let mut out = Stroke::new(s.id, s.level, s.anchor, s.color, s.width);
        out.points = s.points;
        out.order = s.order;
        out.flat_caps = s.flat_caps;
        out.brush = s.brush;
        out
    }
}

/// Version 6: reference images.
#[derive(Deserialize)]
struct FileV6 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV5>,
    images: Vec<crate::image::RefImage>,
}

/// Version 5: strokes have a brush.
#[derive(Deserialize)]
struct FileV5 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV5>,
}

/// Version 4: the camera has a rotation; strokes as in version 3.
#[derive(Deserialize)]
struct FileV4 {
    camera: Camera,
    next_id: StrokeId,
    strokes: Vec<StrokeV3>,
}

/// Stroke of versions 3–4 (no brush).
#[derive(Deserialize)]
struct StrokeV3 {
    id: StrokeId,
    level: i32,
    anchor: [num_bigint::BigInt; 2],
    points: Vec<crate::stroke::StrokePoint>,
    color: [u8; 4],
    width: f32,
    order: u64,
    flat_caps: [bool; 2],
}

impl From<StrokeV3> for Stroke {
    fn from(s: StrokeV3) -> Self {
        let mut out = Stroke::new(s.id, s.level, s.anchor, s.color, s.width);
        out.points = s.points;
        out.order = s.order;
        out.flat_caps = s.flat_caps;
        out
    }
}

/// Camera of versions 1–3 (no rotation).
#[derive(Deserialize)]
struct CameraV1 {
    center: crate::coords::WorldPos,
    log_zoom: f64,
}

impl From<CameraV1> for Camera {
    fn from(c: CameraV1) -> Self {
        Camera { center: c.center, log_zoom: c.log_zoom, rotation: 0.0 }
    }
}

/// Version 3: strokes carry flat caps where the eraser cut them (`Stroke::flat_caps`).
#[derive(Deserialize)]
struct FileV3 {
    camera: CameraV1,
    next_id: StrokeId,
    strokes: Vec<StrokeV3>,
}

/// Version 2: strokes carry a draw order (`Stroke::order`), no caps.
#[derive(Deserialize)]
struct FileV2 {
    camera: CameraV1,
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
    camera: CameraV1,
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
    let file = FileV8 {
        camera: camera.clone(),
        next_id: doc.next_id(),
        strokes: doc.strokes().cloned().collect(),
        images: doc.images().cloned().collect(),
        bookmarks: doc.bookmarks.clone(),
        layers: doc.layers.clone(),
    };
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
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera.into()))
        }
        2 => {
            let f: FileV2 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera.into()))
        }
        3 => {
            let f: FileV3 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera.into()))
        }
        4 => {
            let f: FileV4 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera))
        }
        5 => {
            let f: FileV5 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            Ok((Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id), f.camera))
        }
        6 => {
            let f: FileV6 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            let mut doc = Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id);
            for img in f.images {
                doc.insert_image(img);
            }
            Ok((doc, f.camera))
        }
        7 => {
            let f: FileV7 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            let mut doc = Document::from_strokes(f.strokes.into_iter().map(Stroke::from).collect(), f.next_id);
            for img in f.images {
                doc.insert_image(img);
            }
            doc.bookmarks = f.bookmarks;
            Ok((doc, f.camera))
        }
        8 => {
            let f: FileV8 = postcard::from_bytes(&bytes[6..]).map_err(LoadError::Corrupt)?;
            let mut doc = Document::from_strokes(f.strokes, f.next_id);
            for img in f.images {
                doc.insert_image(img);
            }
            doc.bookmarks = f.bookmarks;
            if !f.layers.is_empty() {
                doc.layers = f.layers;
            }
            Ok((doc, f.camera))
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
        // (the stroke below also checks that the brush survives a round trip)
        cam.rotate_at([1.0, 2.0], [100.0, 100.0], 0.7);
        let anchor = [BigInt::from(-7) << 400u32, BigInt::from(12345) << 333u32];
        let mut s = Stroke::new(doc.alloc_id(), 333, anchor, [1, 2, 3, 255], 3.5);
        s.brush = crate::stroke::Brush::Marker;
        s.push(StrokePoint { x: 1.0, y: -2.0, pressure: 0.5 });
        s.push(StrokePoint { x: 5.0, y: 2.0, pressure: 0.7 });
        doc.insert(s.clone());
        let img = crate::image::RefImage::place(doc.alloc_id(), &cam, [100.0, 100.0], [3, 2], std::sync::Arc::new(vec![1, 2, 3]), 0.8);
        doc.insert_image(img.clone());
        doc.bookmarks.push(crate::flight::Bookmark { name: "Głęboko".into(), camera: cam.clone() });
        let l = doc.add_layer("Tusz");
        doc.layers[0].opacity = 0.5;
        let mut s2 = Stroke::new(doc.alloc_id(), 0, [BigInt::from(1), BigInt::from(1)], [0, 0, 0, 255], 1.0);
        s2.layer = l;
        s2.push(StrokePoint { x: 0.0, y: 0.0, pressure: 1.0 });
        doc.insert(s2.clone());
        let bytes = encode(&doc, &cam);
        let (d2, c2) = decode(&bytes).unwrap();
        assert_eq!(c2, cam);
        assert_eq!(d2.get(s.id), Some(&s));
        assert_eq!(d2.image(img.id), Some(&img));
        assert_eq!(d2.bookmarks, doc.bookmarks);
        assert_eq!(d2.layers, doc.layers);
        assert_eq!(d2.get(s2.id).unwrap().layer, l);
        assert_eq!(d2.next_id(), doc.next_id());

        let dir = std::env::temp_dir().join(format!("icnv-test-{}", std::process::id()));
        let path = dir.join("a.icnv");
        save_file(&path, &doc, &cam).unwrap();
        assert_eq!(load_file(&path).unwrap().0.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(decode(b"nope"), Err(LoadError::BadMagic)));
        assert!(matches!(decode(b"ICNV\x09\x00"), Err(LoadError::UnsupportedVersion(9))));
        assert!(matches!(decode(b"ICNV\x08\x00\xff"), Err(LoadError::Corrupt(_))));
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
        struct C1 {
            center: crate::coords::WorldPos,
            log_zoom: f64,
        }
        #[derive(Serialize)]
        struct F1 {
            camera: C1,
            next_id: StrokeId,
            strokes: Vec<S1>,
        }
        let pts = vec![StrokePoint { x: 1.0, y: 2.0, pressure: 1.0 }, StrokePoint { x: 3.0, y: 2.0, pressure: 1.0 }];
        let f = F1 {
            camera: C1 { center: crate::coords::WorldPos::origin(), log_zoom: 2.5 },
            next_id: 8,
            strokes: vec![S1 { id: 7, level: -3, anchor: [BigInt::from(5), BigInt::from(-5)], points: pts.clone(), color: [9, 9, 9, 255], width: 2.0 }],
        };
        let mut bytes = b"ICNV\x01\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, _) = decode(&bytes).unwrap();
        let s = d.get(7).unwrap();
        assert_eq!((s.order, s.level, &s.points, s.width), (7, -3, &pts, 2.0));
        assert_eq!(decode(&bytes).unwrap().1.log_zoom, 2.5);
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
        struct C1 {
            center: crate::coords::WorldPos,
            log_zoom: f64,
        }
        #[derive(Serialize)]
        struct F2 {
            camera: C1,
            next_id: StrokeId,
            strokes: Vec<S2>,
        }
        let pts = vec![StrokePoint { x: 1.0, y: 2.0, pressure: 1.0 }, StrokePoint { x: 3.0, y: 2.0, pressure: 1.0 }];
        let f = F2 {
            camera: C1 { center: crate::coords::WorldPos::origin(), log_zoom: 0.0 },
            next_id: 10,
            strokes: vec![S2 { id: 9, level: 4, anchor: [BigInt::from(1), BigInt::from(2)], points: pts.clone(), color: [1, 2, 3, 255], width: 5.0, order: 3 }],
        };
        let mut bytes = b"ICNV\x02\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, _) = decode(&bytes).unwrap();
        let s = d.get(9).unwrap();
        assert_eq!((s.order, s.flat_caps, &s.points), (3, [false, false], &pts));
    }

    #[test]
    fn reads_version_3() {
        #[derive(Serialize)]
        struct C1 {
            center: crate::coords::WorldPos,
            log_zoom: f64,
        }
        #[derive(Serialize)]
        struct S3 {
            id: StrokeId,
            level: i32,
            anchor: [BigInt; 2],
            points: Vec<StrokePoint>,
            color: [u8; 4],
            width: f32,
            order: u64,
            flat_caps: [bool; 2],
        }
        #[derive(Serialize)]
        struct F3 {
            camera: C1,
            next_id: StrokeId,
            strokes: Vec<S3>,
        }
        let mut st = Stroke::new(4, 1, [BigInt::from(3), BigInt::from(-3)], [5, 5, 5, 255], 1.5);
        st.push(StrokePoint { x: 0.0, y: 0.0, pressure: 1.0 });
        st.push(StrokePoint { x: 2.0, y: 1.0, pressure: 0.5 });
        st.flat_caps = [true, false];
        let s3 = S3 {
            id: st.id,
            level: st.level,
            anchor: st.anchor.clone(),
            points: st.points.clone(),
            color: st.color,
            width: st.width,
            order: st.order,
            flat_caps: st.flat_caps,
        };
        let f = F3 { camera: C1 { center: crate::coords::WorldPos::origin(), log_zoom: -7.0 }, next_id: 5, strokes: vec![s3] };
        let mut bytes = b"ICNV\x03\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, cam) = decode(&bytes).unwrap();
        assert_eq!(d.get(4), Some(&st));
        assert_eq!((cam.log_zoom, cam.rotation), (-7.0, 0.0));
    }

    #[test]
    fn reads_version_7() {
        #[derive(Serialize)]
        struct S5 {
            id: StrokeId,
            level: i32,
            anchor: [BigInt; 2],
            points: Vec<StrokePoint>,
            color: [u8; 4],
            width: f32,
            order: u64,
            flat_caps: [bool; 2],
            brush: crate::stroke::Brush,
        }
        #[derive(Serialize)]
        struct F7 {
            camera: Camera,
            next_id: StrokeId,
            strokes: Vec<S5>,
            images: Vec<crate::image::RefImage>,
            bookmarks: Vec<crate::flight::Bookmark>,
        }
        let pts = vec![StrokePoint { x: 1.0, y: 2.0, pressure: 1.0 }, StrokePoint { x: 3.0, y: 4.0, pressure: 0.5 }];
        let f = F7 {
            camera: Camera::default(),
            next_id: 3,
            strokes: vec![S5 {
                id: 2,
                level: 0,
                anchor: [BigInt::from(0), BigInt::from(0)],
                points: pts.clone(),
                color: [1, 2, 3, 255],
                width: 2.0,
                order: 2,
                flat_caps: [false, true],
                brush: crate::stroke::Brush::Marker,
            }],
            images: vec![],
            bookmarks: vec![crate::flight::Bookmark { name: "a".into(), camera: Camera::default() }],
        };
        let mut bytes = b"ICNV\x07\x00".to_vec();
        bytes.extend(postcard::to_allocvec(&f).unwrap());
        let (d, _) = decode(&bytes).unwrap();
        let s = d.get(2).unwrap();
        assert_eq!((s.brush, s.flat_caps, s.layer, &s.points), (crate::stroke::Brush::Marker, [false, true], 0, &pts));
        assert_eq!(d.bookmarks.len(), 1);
        assert_eq!(d.layers.len(), 1, "a default layer");
    }
}
