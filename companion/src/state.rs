//! Persistent companion state and game-process detection.
//!
//! The addon owns the font-slot counter (it survives `/reload` in
//! SavedVariables). The companion only needs to know whether the game client
//! has *restarted*, because a fresh process means every font slot is unloaded
//! and the bank can safely be reused from the beginning.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// State persisted between companion runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// PID of the WoW client the last time the companion ran.
    pub wow_pid: Option<u32>,
    /// Set when a client restart was detected; the companion then sends a
    /// `RESET` reply so the addon rewinds its slot counter.
    pub pending_reset: bool,
    /// Monotonic revision stamped into every reply packet.
    pub revision: u32,
}

impl State {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Reconcile against the currently running client.
    ///
    /// Returns `true` when a restart was newly detected.
    pub fn observe_client(&mut self, current_pid: Option<u32>) -> bool {
        match (self.wow_pid, current_pid) {
            (Some(old), Some(new)) if old != new => {
                self.wow_pid = Some(new);
                self.pending_reset = true;
                true
            }
            (None, Some(new)) => {
                self.wow_pid = Some(new);
                false
            }
            _ => false,
        }
    }

    /// Next revision value.
    pub fn bump_revision(&mut self) -> u32 {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.revision
    }
}

/// Locate the running WoW client process, if any.
pub fn find_wow_pid() -> Option<u32> {
    #[cfg(not(windows))]
    {
        let output = Command::new("pgrep")
            .arg("-f")
            .arg("World of Warcraft")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().find_map(|line| line.trim().parse::<u32>().ok())
    }
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Wow.exe", "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        // "Wow.exe","1234",...
        for line in text.lines() {
            let mut parts = line.split(',');
            let name = parts.next().unwrap_or("").trim_matches('"');
            let pid = parts.next().unwrap_or("").trim_matches('"');
            if name.eq_ignore_ascii_case("Wow.exe") {
                if let Ok(pid) = pid.parse::<u32>() {
                    return Some(pid);
                }
            }
        }
        None
    }
}

/// Default path for the state file, next to the config file.
pub fn default_state_path() -> PathBuf {
    crate::config::default_config_path()
        .parent()
        .map(|p| p.join("state.json"))
        .unwrap_or_else(|| PathBuf::from("ocw-state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_restart() {
        let mut state = State::default();
        assert!(!state.observe_client(Some(10)));
        assert!(!state.observe_client(Some(10)));
        assert!(state.observe_client(Some(11)));
        assert!(state.pending_reset);
    }

    #[test]
    fn revision_is_monotonic_and_nonzero() {
        let mut state = State::default();
        assert_eq!(state.bump_revision(), 1);
        assert_eq!(state.bump_revision(), 2);
    }
}
