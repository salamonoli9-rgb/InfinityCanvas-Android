//! Platform-independent core of the infinite canvas: exact coordinates, camera,
//! strokes, spatial index, undo history and file format. No GPU code here.

pub mod camera;
pub mod coords;
pub mod document;
pub mod flight;
pub mod history;
pub mod image;
pub mod io;
pub mod mesh;
pub mod posterize;
pub mod smoothing;
pub mod stroke;
pub mod trace;
