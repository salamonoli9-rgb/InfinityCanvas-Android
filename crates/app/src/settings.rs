//! App settings chosen on the first start (and later in More → Settings): language and the
//! kind of device, which sets the size of the interface.

use crate::i18n::Lang;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Phone,
    Tablet,
}

impl Device {
    /// Size of the interface relative to the phone layout.
    pub fn ui_scale(self) -> f32 {
        match self {
            Device::Phone => 1.0,
            Device::Tablet => 1.25,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub lang: Lang,
    pub device: Device,
}

const FILE: &str = "settings.txt";

impl Settings {
    /// Settings saved in `dir`, or `None` on the first start.
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
        let mut s = Settings { lang: Lang::Pl, device: Device::Phone };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("lang", v)) => s.lang = Lang::from_code(v.trim()),
                Some(("device", v)) => s.device = if v.trim() == "tablet" { Device::Tablet } else { Device::Phone },
                _ => {}
            }
        }
        Some(s)
    }

    pub fn save(&self, dir: &Path) {
        let device = match self.device {
            Device::Phone => "phone",
            Device::Tablet => "tablet",
        };
        let _ = std::fs::create_dir_all(dir);
        if let Err(e) = std::fs::write(dir.join(FILE), format!("lang={}\ndevice={device}\n", self.lang.code())) {
            log::warn!("settings not saved: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("icnv-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(Settings::load(&dir).is_none(), "first start");
        let s = Settings { lang: Lang::Ru, device: Device::Tablet };
        s.save(&dir);
        assert_eq!(Settings::load(&dir), Some(s));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
