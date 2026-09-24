//! The font-slot bank: a directory of pre-created, initially identical font
//! files that the companion replaces one at a time.
//!
//! Why a *bank*: the client caches a font after its first load in a given
//! process, and files created after the UI has loaded are not discovered. By
//! pre-creating many filenames before the game starts and only ever replacing
//! *not yet loaded* ones, the companion can send many replies without a reload.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fonts::ttf;

/// Default number of slots created by `ocw install`.
pub const DEFAULT_SLOTS: u16 = 4096;

/// File name for a bank slot, e.g. `fontreply0042.ttf`.
pub fn slot_filename(slot: u16) -> String {
    format!("fontreply{slot:04}.ttf")
}

/// Handle to an installed font bank on disk.
#[derive(Debug, Clone)]
pub struct Bank {
    dir: PathBuf,
    slots: u16,
}

impl Bank {
    /// Open (or describe) a bank directory. The directory need not exist yet.
    pub fn new(dir: impl Into<PathBuf>, slots: u16) -> Self {
        Self {
            dir: dir.into(),
            slots,
        }
    }

    /// Directory holding the bank.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Number of configured slots.
    pub fn slots(&self) -> u16 {
        self.slots
    }

    /// Path of a slot file.
    pub fn slot_path(&self, slot: u16) -> PathBuf {
        self.dir.join(slot_filename(slot))
    }

    /// Whether the bank has already been installed.
    pub fn is_installed(&self) -> bool {
        self.slot_path(1).is_file() && self.slot_path(self.slots).is_file()
    }

    /// Create every slot file, hard-linking them to a single baseline font when
    /// the filesystem supports it, and copying otherwise.
    ///
    /// Existing slot files are left untouched so that a used bank is never
    /// clobbered while the game is running.
    pub fn install(&self, force: bool) -> io::Result<InstallReport> {
        fs::create_dir_all(&self.dir)?;

        let baseline_path = self.dir.join("baseline.ttf");
        fs::write(&baseline_path, ttf::build_baseline_font())?;

        let mut created = 0u32;
        let mut hard_linked = 0u32;
        let mut copied = 0u32;

        for slot in 1..=self.slots {
            let path = self.slot_path(slot);
            if path.exists() && !force {
                continue;
            }
            let _ = fs::remove_file(&path);
            match fs::hard_link(&baseline_path, &path) {
                Ok(()) => hard_linked += 1,
                Err(_) => {
                    fs::copy(&baseline_path, &path)?;
                    copied += 1;
                }
            }
            created += 1;
        }

        Ok(InstallReport {
            dir: self.dir.clone(),
            slots: self.slots,
            created,
            hard_linked,
            copied,
        })
    }

    /// Atomically publish `bytes` into `slot`.
    ///
    /// A hard-linked slot is detached by the rename, so sibling slots sharing
    /// the baseline file are unaffected.
    pub fn publish(&self, slot: u16, bytes: &[u8]) -> io::Result<()> {
        let target = self.slot_path(slot);
        let tmp = self.dir.join(format!("{}.tmp", slot_filename(slot)));
        fs::write(&tmp, bytes)?;
        // rename(2) replaces atomically on the same filesystem.
        fs::rename(&tmp, &target)
    }

    /// Publish a reply packet into `slot`.
    pub fn publish_reply(&self, slot: u16, packet: &[u8; 512]) -> io::Result<()> {
        self.publish(slot, &ttf::build_reply_font(packet))
    }

    /// Reset every slot to the baseline font. Only safe while the client is
    /// stopped, or immediately after it has restarted.
    pub fn reset(&self) -> io::Result<()> {
        let baseline = ttf::build_baseline_font();
        for slot in 1..=self.slots {
            self.publish(slot, &baseline)?;
        }
        Ok(())
    }

    /// Read back the 512 reply bytes stored in `slot`, for diagnostics.
    pub fn read_reply(&self, slot: u16) -> io::Result<Vec<u8>> {
        let font = fs::read(self.slot_path(slot))?;
        let advances = ttf::read_advances(&font);
        let low = advances[ttf::GLYPH_CAL_LOW as usize];
        let high = advances[ttf::GLYPH_CAL_HIGH as usize];
        let bytes = (0..ttf::DATA_GLYPHS)
            .map(|i| ttf::decode_byte(low, high, advances[ttf::GLYPH_DATA_BASE as usize + i]))
            .collect();
        Ok(bytes)
    }
}

/// Summary returned by [`Bank::install`].
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub dir: PathBuf,
    pub slots: u16,
    pub created: u32,
    pub hard_linked: u32,
    pub copied: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frames::{reply_state, ReplyFrame};

    fn temp_bank(name: &str, slots: u16) -> Bank {
        let dir = std::env::temp_dir().join(format!("ocw-bank-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Bank::new(dir, slots)
    }

    #[test]
    fn installs_and_publishes() {
        let bank = temp_bank("publish", 8);
        let report = bank.install(false).unwrap();
        assert_eq!(report.created, 8);
        assert!(bank.is_installed());

        let frame = ReplyFrame {
            state: reply_state::DONE,
            ui_session: 5,
            request_id: 99,
            fragment_index: 1,
            fragment_count: 1,
            revision: 7,
            slot: 3,
            flags: 0,
            payload: b"hello from the companion".to_vec(),
        };
        let packet = frame.encode();
        bank.publish_reply(3, &packet).unwrap();

        let recovered = bank.read_reply(3).unwrap();
        assert_eq!(ReplyFrame::decode(&recovered).unwrap(), frame);

        // A sibling slot must still hold the baseline.
        let sibling = bank.read_reply(4).unwrap();
        assert_eq!(ReplyFrame::decode(&sibling), Err(crate::protocol::frames::DecodeError::BadMagic));

        let _ = fs::remove_dir_all(bank.dir());
    }

    #[test]
    fn install_is_idempotent() {
        let bank = temp_bank("idempotent", 4);
        bank.install(false).unwrap();
        let second = bank.install(false).unwrap();
        assert_eq!(second.created, 0);
        let _ = fs::remove_dir_all(bank.dir());
    }

    #[test]
    fn slot_filenames_are_zero_padded() {
        assert_eq!(slot_filename(1), "fontreply0001.ttf");
        assert_eq!(slot_filename(65535), "fontreply65535.ttf");
    }
}
