//! Screen capture.
//!
//! Capture is delegated to an external command so the companion stays
//! dependency-free and portable. A command template receives the region and an
//! output path; the resulting PNG is decoded by [`crate::capture::png`].
//!
//! macOS gets a working default (`screencapture`). Windows and Linux users
//! supply `--capture-cmd`; see `docs/setup.md` for ready-made templates.

use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::capture::png::{self, DecodedImage};

/// The screen rectangle to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// A region covering the whole screen.
    pub fn full_screen() -> Self {
        Region {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    /// Whether this region means "the whole screen".
    pub fn is_full_screen(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A configured screen capturer.
#[derive(Debug, Clone)]
pub struct Capturer {
    pub region: Region,
    /// Optional command template overriding the platform default.
    pub command: Option<String>,
    /// Cell size hint used only for reporting; decoding auto-detects it.
    pub cell_px: u32,
}

impl Capturer {
    pub fn new(region: Region, command: Option<String>, cell_px: u32) -> Self {
        Self {
            region,
            command,
            cell_px,
        }
    }

    /// Capture the configured region and decode it to luminance.
    pub fn capture(&self) -> Result<DecodedImage> {
        let out = std::env::temp_dir().join(format!(
            "ocw-capture-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&out);

        let command = match &self.command {
            Some(template) => substitute(template, &self.region, &out),
            None => default_command(&self.region, &out)?,
        };

        let status = shell_command(&command)
            .status()
            .with_context(|| format!("failed to run capture command: {command}"))?;
        if !status.success() {
            bail!("capture command failed with {status}: {command}");
        }

        let bytes = std::fs::read(&out)
            .with_context(|| format!("capture command produced no file at {}", out.display()))?;
        let _ = std::fs::remove_file(&out);
        png::decode_png(&bytes)
    }
}

fn substitute(template: &str, region: &Region, out: &std::path::Path) -> String {
    template
        .replace("{x}", &region.x.to_string())
        .replace("{y}", &region.y.to_string())
        .replace("{w}", &region.width.to_string())
        .replace("{h}", &region.height.to_string())
        .replace("{x2}", &(region.x + region.width as i32).to_string())
        .replace("{y2}", &(region.y + region.height as i32).to_string())
        .replace("{out}", &out.display().to_string())
}

fn default_command(region: &Region, out: &std::path::Path) -> Result<String> {
    let out = out.display();

    #[cfg(target_os = "macos")]
    {
        if region.is_full_screen() {
            return Ok(format!("screencapture -x -t png \"{out}\""));
        }
        return Ok(format!(
            "screencapture -x -t png -R{},{},{},{} \"{out}\"",
            region.x, region.y, region.width, region.height
        ));
    }

    #[cfg(target_os = "linux")]
    {
        if region.is_full_screen() {
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
        let cmd = substitute("cap {x},{y} {w}x{h} -> {out} [{x2},{y2}]", &region, out);
        assert_eq!(cmd, "cap 8,16 512x16 -> /tmp/x.png [520,32]");
    }
}
