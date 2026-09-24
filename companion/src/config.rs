//! Companion configuration: where the addon lives, how to capture the strip,
//! and how to reach the OpenCode server.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fonts::bank::DEFAULT_SLOTS;

/// Root configuration object, serialised as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Directory of the installed `OCWow` addon (contains `Fonts/`).
    pub addon_dir: PathBuf,
    /// Number of font-bank slots created by `ocw install`.
    pub bank_slots: u16,
    /// Screen-capture settings for the pixel strip.
    pub capture: CaptureSettings,
    /// OpenCode server settings.
    pub opencode: OpenCodeSettings,
    /// Root folder whose sub-directories are offered as projects.
    pub projects_root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addon_dir: detect_addon_dir().unwrap_or_else(|| {
                PathBuf::from("Interface/AddOns/OCWow")
            }),
            bank_slots: DEFAULT_SLOTS,
            capture: CaptureSettings::default(),
            opencode: OpenCodeSettings::default(),
            projects_root: None,
        }
    }
}

impl Config {
    /// Load configuration from `path`, falling back to defaults.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Write configuration to `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The font bank for this configuration.
    pub fn bank(&self) -> crate::fonts::Bank {
        crate::fonts::Bank::new(self.addon_dir.join("Fonts"), self.bank_slots)
    }
}

/// Where the pixel strip is on screen, and how to grab it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Cell edge length in screen pixels; `0` means auto-detect.
    pub cell_px: u32,
    /// Optional capture command template (see `docs/setup.md`).
    pub command: Option<String>,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            x: 8,
            y: 8,
            width: crate::protocol::pixel::STRIP_COLS as u32
                * crate::protocol::pixel::DEFAULT_CELL_PX,
            height: crate::protocol::pixel::STRIP_ROWS as u32
                * crate::protocol::pixel::DEFAULT_CELL_PX,
            // 0 means "derive the cell size from the captured width", which is
            // required for Retina and otherwise scaled displays.
            cell_px: 0,
            command: None,
        }
    }
}

impl CaptureSettings {
    pub fn region(&self) -> crate::capture::Region {
        crate::capture::Region {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// How to reach and drive the OpenCode server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenCodeSettings {
    /// Base URL of the OpenCode HTTP API.
    pub base_url: String,
    /// Password for the local background service, if any.
    pub password: Option<String>,
    /// Default project directory for new sessions.
    pub project: Option<PathBuf>,
    /// Default model reference (`provider/model`).
    pub model: Option<String>,
    /// Request timeout for prompt calls, in seconds.
    pub timeout_secs: u64,
}

impl Default for OpenCodeSettings {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:4096".to_string(),
            password: read_service_password(),
            project: None,
            model: None,
            timeout_secs: 900,
        }
    }
}

/// Read the shared secret the OpenCode background service expects, if present.
pub fn read_service_password() -> Option<String> {
    read_service_registration().map(|(_, password)| password)
}

/// Read the running service's URL and password from its registration file.
///
/// The background service writes `~/.local/state/opencode/service.json` (or the
/// platform equivalent) with the dynamic port it bound to. Preferring this over
/// a fixed port means the companion follows whatever the user already runs.
pub fn read_service_registration() -> Option<(String, String)> {
    let mut candidates = Vec::new();
    if let Some(home) = home_dir() {
        candidates.push(home.join(".local/state/opencode/service.json"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = home_dir() {
        candidates.push(home.join("Library/Application Support/opencode/service.json"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("opencode").join("service.json"));
    }

    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let url = value.get("url").and_then(|v| v.as_str());
        let password = value.get("password").and_then(|v| v.as_str());
        if let (Some(url), Some(password)) = (url, password) {
            return Some((url.to_string(), password.to_string()));
        }
    }
    None
}

/// The user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Default location of the companion configuration file.
pub fn default_config_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("ocw").join("config.json");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("ocw").join("config.json");
        }
        if let Some(home) = home_dir() {
            return home.join(".config").join("ocw").join("config.json");
        }
    }
    PathBuf::from("ocw-config.json")
}

/// Best-effort discovery of an existing WoW `AddOns` directory.
pub fn detect_addon_dir() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        #[cfg(target_os = "macos")]
        {
            for flavor in ["_classic_beta_", "_retail_", "_classic_", "_classic_era_"] {
                v.push(
                    PathBuf::from("/Applications/World of Warcraft")
                        .join(flavor)
                        .join("Interface/AddOns/OCWow"),
                );
            }
        }
        #[cfg(target_os = "windows")]
        {
            for base in [
                r"C:\Program Files (x86)\World of Warcraft",
                r"C:\Program Files\World of Warcraft",
                r"C:\World of Warcraft",
            ] {
                for flavor in ["_classic_beta_", "_retail_", "_classic_", "_classic_era_"] {
                    v.push(PathBuf::from(base).join(flavor).join("Interface/AddOns/OCWow"));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if let Some(home) = home_dir() {
                for base in [
                    home.join(".steam/steam/steamapps/common/World of Warcraft"),
                    home.join(".local/share/Steam/steamapps/common/World of Warcraft"),
                ] {
                    for flavor in ["_classic_beta_", "_retail_", "_classic_", "_classic_era_"] {
                        v.push(base.join(flavor).join("Interface/AddOns/OCWow"));
                    }
                }
            }
        }
        v
    };

    candidates
        .into_iter()
        .find(|p| p.parent().map(|a| a.is_dir()).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips() {
        let mut cfg = Config::default();
        cfg.capture.x = 100;
        cfg.bank_slots = 32;
        let dir = std::env::temp_dir().join(format!("ocw-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.capture.x, 100);
        assert_eq!(loaded.bank_slots, 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_config_yields_defaults() {
        let cfg = Config::load(Path::new("/definitely/not/here.json")).unwrap();
        assert!(cfg.bank_slots > 0);
    }
}
