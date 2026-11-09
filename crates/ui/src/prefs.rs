//! UI preferences persisted per user.
//!
//! Location: `%LOCALAPPDATA%\core-pulse\ui-prefs.txt` (key=value lines).
//! All defaults fail closed: bridge stays off unless the user enabled it in
//! the UI or passed `--bridge` / `CORE_PULSE_LHM_BRIDGE=1` at launch.
//!
//! Format is intentionally tiny (no serde dependency in the UI crate):
//!
//! ```text
//! bridge_enabled=false
//! show_system=false
//! show_signatures=false
//! sensor_hot_only=false
//! process_density=compact
//! col_path=true
//! col_io=true
//! col_gpu=true
//! col_threads=true
//! col_handles=true
//! col_session=true
//! ```

use std::path::PathBuf;

/// Persisted UI preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPrefs {
    pub bridge_enabled: bool,
    pub show_system: bool,
    pub show_signatures: bool,
    pub sensor_hot_only: bool,
    /// true = Comfort, false = Compact.
    pub comfort: bool,
    pub col_path: bool,
    pub col_io: bool,
    pub col_gpu: bool,
    pub col_threads: bool,
    pub col_handles: bool,
    pub col_session: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            bridge_enabled: false,
            show_system: false,
            show_signatures: false,
            sensor_hot_only: false,
            comfort: false,
            col_path: true,
            col_io: true,
            col_gpu: true,
            col_threads: true,
            col_handles: true,
            col_session: true,
        }
    }
}

impl UiPrefs {
    /// Path to the prefs file under `%LOCALAPPDATA%\core-pulse\ui-prefs.txt`.
    pub fn path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Temp"));
        base.join("core-pulse").join("ui-prefs.txt")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let mut prefs = UiPrefs::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return prefs;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let raw = v.trim();
            let b = raw.eq_ignore_ascii_case("true");
            if k == "process_density" {
                prefs.comfort = raw.eq_ignore_ascii_case("comfort");
                continue;
            }
            match k {
                "bridge_enabled" => prefs.bridge_enabled = b,
                "show_system" => prefs.show_system = b,
                "show_signatures" => prefs.show_signatures = b,
                "sensor_hot_only" => prefs.sensor_hot_only = b,
                "col_path" => prefs.col_path = b,
                "col_io" => prefs.col_io = b,
                "col_gpu" => prefs.col_gpu = b,
                "col_threads" => prefs.col_threads = b,
                "col_handles" => prefs.col_handles = b,
                "col_session" => prefs.col_session = b,
                _ => {}
            }
        }
        prefs
    }

    pub fn save(&self) {
        let _ = self.save_to(&Self::path());
    }

    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let density = if self.comfort { "comfort" } else { "compact" };
        let text = format!(
            "bridge_enabled={}\n\
             show_system={}\n\
             show_signatures={}\n\
             sensor_hot_only={}\n\
             process_density={density}\n\
             col_path={}\n\
             col_io={}\n\
             col_gpu={}\n\
             col_threads={}\n\
             col_handles={}\n\
             col_session={}\n",
            self.bridge_enabled,
            self.show_system,
            self.show_signatures,
            self.sensor_hot_only,
            self.col_path,
            self.col_io,
            self.col_gpu,
            self.col_threads,
            self.col_handles,
            self.col_session,
        );
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_fail_closed() {
        let prefs = UiPrefs::default();
        assert!(!prefs.bridge_enabled);
        assert!(!prefs.comfort);
        assert!(prefs.col_path);
    }

    #[test]
    fn roundtrip_preserves_prefs() {
        let dir = std::env::temp_dir().join(format!("cp-prefs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui-prefs.txt");
        let prefs = UiPrefs {
            bridge_enabled: true,
            show_system: true,
            show_signatures: true,
            sensor_hot_only: true,
            comfort: true,
            col_path: false,
            col_io: false,
            col_gpu: false,
            col_threads: false,
            col_handles: false,
            col_session: false,
        };
        prefs.save_to(&path).unwrap();
        let loaded = UiPrefs::load_from(&path);
        assert_eq!(loaded, prefs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_default() {
        let p = UiPrefs::load_from(std::path::Path::new("C:\\does-not-exist-cp\\ui-prefs.txt"));
        assert_eq!(p, UiPrefs::default());
    }
}
