//! Screen capture.
//!
//! Capture is delegated to an external command so the companion stays
//! dependency-free and portable. A command template receives the region and an
//! output path; the resulting PNG is decoded by [`crate::capture::png`].
//!
//! macOS gets a working default (`screencapture`). Windows and Linux users
//! supply `--capture-cmd`; see `docs/setup.md`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::capture::png::{self, RgbImage};

/// A rectangle in screen **points** (not pixels). On a Retina display a capture
/// of `w` points yields an image `w * scale` pixels wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// The whole screen.
    pub fn full() -> Self {
        Region {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn is_full(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Captures screenshots by shelling out to a platform command.
#[derive(Debug, Clone)]
pub struct Capturer {
    /// Optional command template overriding the platform default.
    pub command: Option<String>,
}

impl Default for Capturer {
    fn default() -> Self {
        Self { command: None }
    }
}

impl Capturer {
    pub fn new(command: Option<String>) -> Self {
        Self { command }
    }

    /// Capture a region, or the whole screen when `region` is `None`/full.
    pub fn capture(&self, region: Region) -> Result<RgbImage> {
        let out = temp_path();
        let _ = std::fs::remove_file(&out);

        let command = match &self.command {
            Some(template) => substitute(template, region, &out),
            None => default_command(region, &out)?,
        };

        let status = shell_command(&command)
            .status()
            .with_context(|| format!("failed to run capture command: {command}"))?;
        if !status.success() {
            bail!("capture command failed with {status}: {command}");
        }

        let bytes = std::fs::read(&out)
            .with_context(|| format!("capture produced no file at {}", out.display()))?;
        let _ = std::fs::remove_file(&out);
        png::decode_png(&bytes)
    }

    /// Capture the whole screen.
    pub fn capture_full(&self) -> Result<RgbImage> {
        self.capture(Region::full())
    }

    /// How many image pixels one screen point becomes (2 on a Retina display).
    ///
    /// Determined empirically so the caller can convert a location found in an
    /// image back into a screen rectangle.
    pub fn point_scale(&self) -> Result<f64> {
        const PROBE: u32 = 64;
        let image = self.capture(Region {
            x: 0,
            y: 0,
            width: PROBE,
            height: PROBE,
        })?;
        if image.width == 0 {
            bail!("capture returned an empty image");
        }
        Ok(image.width as f64 / PROBE as f64)
    }
}

fn temp_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ocw-capture-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ))
}

fn substitute(template: &str, region: Region, out: &std::path::Path) -> String {
    template
        .replace("{x}", &region.x.to_string())
        .replace("{y}", &region.y.to_string())
        .replace("{w}", &region.width.to_string())
        .replace("{h}", &region.height.to_string())
        .replace("{x2}", &(region.x + region.width as i32).to_string())
        .replace("{y2}", &(region.y + region.height as i32).to_string())
        .replace("{out}", &out.display().to_string())
}

fn default_command(region: Region, out: &std::path::Path) -> Result<String> {
    let out = out.display();

    #[cfg(target_os = "macos")]
    {
        if region.is_full() {
            return Ok(format!("screencapture -x -t png \"{out}\""));
        }
        return Ok(format!(
            "screencapture -x -t png -R{},{},{},{} \"{out}\"",
            region.x, region.y, region.width, region.height
        ));
    }

    #[cfg(target_os = "linux")]
    {
        if region.is_full() {
            return Ok(format!("grim \"{out}\""));
        }
        return Ok(format!(
            "grim -g \"{},{} {}x{}\" \"{out}\"",
            region.x, region.y, region.width, region.height
        ));
    }

    #[cfg(target_os = "windows")]
    {
        let _ = region;
        bail!(
            "no default capture command on Windows; pass --capture-cmd \
             (see docs/setup.md for a PowerShell template)"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = region;
        bail!("no default capture command for this platform; pass --capture-cmd");
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_placeholders() {
        let region = Region {
            x: 8,
            y: 16,
            width: 512,
            height: 16,
        };
        let out = std::path::Path::new("/tmp/x.png");
        let cmd = substitute("cap {x},{y} {w}x{h} -> {out} [{x2},{y2}]", region, out);
        assert_eq!(cmd, "cap 8,16 512x16 -> /tmp/x.png [520,32]");
    }
}
