//! Companion configuration: where the addon lives, where the strip is on
//! screen, and how to reach the OpenCode server.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::slots::{SlotBank, DEFAULT_SLOTS};

/// Interface version written into generated slot addons.
pub const DEFAULT_INTERFACE: u32 = 16001;

/// Root configuration object, serialised as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Directory of the installed `OCWow` addon (`…/Interface/AddOns/OCWow`).
    pub addon_dir: PathBuf,
    /// Number of load-on-demand slot addons created by `ocw install`.
    pub slots: u16,
    /// `## Interface:` written into generated slot addons.
    pub interface: u32,
    /// Screen capture settings.
    pub capture: CaptureSettings,
    /// OpenCode server settings.
    pub opencode: OpenCodeSettings,
    /// Root folder whose sub-directories are offered as projects.
    pub projects_root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addon_dir: detect_addon_dir().unwrap_or_else(|| PathBuf::from("Interface/AddOns/OCWow")),
            slots: DEFAULT_SLOTS,
            interface: DEFAULT_INTERFACE,
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
        let config: Config =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    /// Write configuration to `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The slot bank this configuration describes.
    pub fn slot_bank(&self) -> SlotBank {
        SlotBank::new(self.addon_dir.clone(), self.slots)
    }
}

/// Where the strip is on screen, and how to grab it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    /// Optional capture command template (see `docs/setup.md`).
    pub command: Option<String>,
    /// Cached strip location, discovered by `ocw probe`.
    pub strip: Option<StripSettings>,
}

/// A cached screen rectangle, in points, that contains the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StripSettings {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl StripSettings {
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
        let mut config = Config::default();
        config.slots = 32;
        config.capture.strip = Some(StripSettings {
            x: 10,
            y: 20,
            width: 800,
            height: 192,
        });
        let dir = std::env::temp_dir().join(format!("ocw-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.slots, 32);
        assert_eq!(loaded.capture.strip.unwrap().width, 800);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_config_yields_defaults() {
        let config = Config::load(Path::new("/definitely/not/here.json")).unwrap();
        assert_eq!(config.slots, DEFAULT_SLOTS);
    }
}
