//! Projects ("kartki"): one `.icnv` file per drawing in a projects folder.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const EXTENSION: &str = "icnv";
const LAST_FILE: &str = "last_project.txt";
const DEFAULT_BASE: &str = "Rysunek";

#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub name: String,
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

#[derive(Clone)]
pub struct Projects {
    root: PathBuf,
    dir: PathBuf,
}

impl Projects {
    /// Opens (and creates) `<data_dir>/projects`. A drawing from before projects existed
    /// (`<data_dir>/canvas.icnv`) is moved in as the first project.
    pub fn new(data_dir: &Path) -> std::io::Result<Self> {
        let dir = data_dir.join("projects");
        std::fs::create_dir_all(&dir)?;
        let p = Self { root: data_dir.to_path_buf(), dir };
        let legacy = data_dir.join("canvas.icnv");
        if legacy.exists() {
            let name = p.unique_name(DEFAULT_BASE);
            std::fs::rename(&legacy, p.path_for(&name))?;
            p.set_last(&name);
            log::info!("moved the old drawing to project {name:?}");
        }
        Ok(p)
    }

    pub fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{EXTENSION}"))
    }

    /// Preview image of a project (PNG next to the drawing).
    pub fn thumb_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.png"))
    }

    pub fn save_thumbnail(&self, name: &str, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
        let path = self.thumb_path(name);
        let tmp = path.with_extension("png.tmp");
        {
            let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            let mut enc = png::Encoder::new(f, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(std::io::Error::other)?;
            wr.write_image_data(rgba).map_err(std::io::Error::other)?;
        }
        std::fs::rename(tmp, path)
    }

    /// (width, height, RGBA) of the project's preview, if there is one.
    pub fn load_thumbnail(&self, name: &str) -> Option<(u32, u32, Vec<u8>)> {
        let f = std::fs::File::open(self.thumb_path(name)).ok()?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(f));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        buf.truncate(info.buffer_size());
        (info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight).then_some((info.width, info.height, buf))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path_for(name).exists()
    }

    /// All projects, most recently modified first.
    pub fn list(&self) -> Vec<ProjectInfo> {
        let mut out: Vec<ProjectInfo> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()? != EXTENSION {
                    return None;
                }
                let name = path.file_stem()?.to_str()?.to_string();
                let modified = e.metadata().ok().and_then(|m| m.modified().ok());
                Some(ProjectInfo { name, path, modified })
            })
            .collect();
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// `base`, or `base 2`, `base 3`… – the first name not taken yet.
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        if !base.is_empty() && !self.exists(base) && base != DEFAULT_BASE {
            return base.to_string();
        }
        let base = if base.is_empty() { DEFAULT_BASE } else { base };
        (1..).map(|i| format!("{base} {i}")).find(|n| !self.exists(n)).expect("infinite iterator")
    }

    pub fn new_name(&self) -> String {
        self.unique_name(DEFAULT_BASE)
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<String, String> {
        let to = sanitize(to).ok_or("Nieprawidłowa nazwa")?;
        if to == from {
            return Ok(to);
        }
        if self.exists(&to) {
            return Err(format!("Projekt „{to}” już istnieje"));
        }
        let src = self.path_for(from);
        if src.exists() {
            std::fs::rename(src, self.path_for(&to)).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::rename(self.thumb_path(from), self.thumb_path(&to));
        Ok(to)
    }

    pub fn duplicate(&self, name: &str) -> Result<String, String> {
        let copy = self.unique_name(&format!("{name} (kopia)"));
        std::fs::copy(self.path_for(name), self.path_for(&copy)).map_err(|e| e.to_string())?;
        let _ = std::fs::copy(self.thumb_path(name), self.thumb_path(&copy));
        Ok(copy)
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        let _ = std::fs::remove_file(self.thumb_path(name));
        match std::fs::remove_file(self.path_for(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn last(&self) -> Option<String> {
        let name = std::fs::read_to_string(self.root.join(LAST_FILE)).ok()?;
        let name = name.trim().to_string();
        self.exists(&name).then_some(name)
    }

    pub fn set_last(&self, name: &str) {
        if let Err(e) = std::fs::write(self.root.join(LAST_FILE), name) {
            log::warn!("cannot remember the last project: {e}");
        }
    }
}

/// A file-name-safe project name, or `None` if nothing usable is left.
pub fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_start_matches('.').trim().to_string();
    let cleaned: String = cleaned.chars().take(60).collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// "przed chwilą", "5 min temu", "3 godz. temu", "2 dni temu".
pub fn ago(t: Option<SystemTime>) -> String {
    let Some(s) = t.and_then(|t| SystemTime::now().duration_since(t).ok()).map(|d| d.as_secs()) else {
        return String::new();
    };
    match s {
        0..60 => "przed chwilą".into(),
        60..3600 => format!("{} min temu", s / 60),
        3600..86400 => format!("{} godz. temu", s / 3600),
        86400..172800 => "wczoraj".into(),
        _ => format!("{} dni temu", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("icnv-projects-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_and_files() {
        let root = tmp("names");
        std::fs::write(root.join("canvas.icnv"), b"old").unwrap();
        let p = Projects::new(&root).unwrap();
        // Legacy drawing migrated and remembered.
        assert_eq!(p.last().as_deref(), Some("Rysunek 1"));
        assert_eq!(p.new_name(), "Rysunek 2");
        std::fs::write(p.path_for("Rysunek 2"), b"x").unwrap();
        assert_eq!(p.list().len(), 2);

        assert_eq!(p.rename("Rysunek 2", "  Las / góry  ").unwrap(), "Las góry");
        assert!(p.exists("Las góry") && !p.exists("Rysunek 2"));
        assert!(p.rename("Las góry", "Rysunek 1").is_err());
        assert!(p.rename("Las góry", " ../ ").is_err());

        assert_eq!(p.duplicate("Las góry").unwrap(), "Las góry (kopia)");
        assert_eq!(p.duplicate("Las góry").unwrap(), "Las góry (kopia) 1");
        p.delete("Las góry (kopia) 1").unwrap();
        assert_eq!(p.list().len(), 3);

        // Thumbnails follow their project.
        let px: Vec<u8> = (0..64 * 2 * 4).map(|i| i as u8).collect();
        p.save_thumbnail("Las góry", 64, 2, &px).unwrap();
        assert_eq!(p.load_thumbnail("Las góry"), Some((64, 2, px.clone())));
        assert_eq!(p.rename("Las góry", "Góry").unwrap(), "Góry");
        assert!(p.load_thumbnail("Góry").is_some() && p.load_thumbnail("Las góry").is_none());
        p.delete("Góry").unwrap();
        assert!(!p.thumb_path("Góry").exists());
        assert_eq!(p.list().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("  a\tb  c "), Some("a b c".into()));
        assert_eq!(sanitize(".hidden"), Some("hidden".into()));
        assert_eq!(sanitize("..."), None);
        assert_eq!(sanitize("ąęśćżź"), Some("ąęśćżź".into()));
    }
}
