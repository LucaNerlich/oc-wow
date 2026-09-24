//! Fixed-size transport frames.
//!
//! Three frame types travel over the two channels:
//!
//! | Frame | Channel | Size | Purpose |
//! |-------|---------|------|---------|
//! | [`PromptFrame`]  | pixels (addon -> companion) | 64 B  | prompt text fragments |
//! | [`ControlFrame`] | pixels (addon -> companion) | 64 B  | slot/fragment scheduling |
//! | [`ReplyFrame`]   | font metrics (companion -> addon) | 512 B | replies + status |
//!
//! All multi-byte integers are big-endian. Every frame ends with an Adler-32
//! over the preceding bytes.

use crate::protocol::adler32::adler32;
use crate::protocol::pixel::BYTES_PER_FRAME;

/// Magic marking outbound (addon -> companion) pixel frames: `"OC"`.
pub const OUT_MAGIC: [u8; 2] = *b"OC";
/// Magic marking inbound (companion -> addon) font frames: `"CF"`.
pub const IN_MAGIC: [u8; 2] = *b"CF";
/// Protocol version carried by every frame.
pub const PROTOCOL_VERSION: u8 = 1;

/// `type` value for [`PromptFrame`].
pub const OUT_TYPE_PROMPT: u8 = 1;
/// `type` value for [`ControlFrame`].
pub const OUT_TYPE_CONTROL: u8 = 2;

/// Maximum prompt bytes carried by one [`PromptFrame`].
pub const PROMPT_PAYLOAD_MAX: usize = 40;
/// Maximum reply bytes carried by one [`ReplyFrame`].
pub const REPLY_PAYLOAD_MAX: usize = 476;

/// Size of the pixel frame header.
const OUT_HEADER_LEN: usize = 20;
/// Size of a reply frame header.
const IN_HEADER_LEN: usize = 32;
/// Total size of a reply frame.
const REPLY_FRAME_LEN: usize = 512;

/// Reply/status states carried in [`ReplyFrame::state`] and control frames.
pub mod reply_state {
    pub const WAITING: u8 = 0;
    pub const QUEUED: u8 = 1;
    pub const WORKING: u8 = 2;
    pub const STREAMING: u8 = 3;
    pub const DONE: u8 = 4;
    pub const FAILED: u8 = 5;
    pub const INTERRUPTED: u8 = 6;
    /// Instruction to the addon to reset its font-slot counter (client restarted).
    pub const RESET: u8 = 7;

    pub fn name(state: u8) -> &'static str {
        match state {
            WAITING => "waiting",
            QUEUED => "queued",
            WORKING => "working",
            STREAMING => "streaming",
            DONE => "done",
            FAILED => "failed",
            INTERRUPTED => "interrupted",
            RESET => "reset",
            _ => "unknown",
        }
    }
}

/// Errors from decoding a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The frame length did not match the expected size.
    BadLength { expected: usize, got: usize },
    /// The leading magic bytes did not match.
    BadMagic,
    /// The `version` byte was not [`PROTOCOL_VERSION`].
    BadVersion(u8),
    /// The `type` byte was not a known frame type.
    BadType(u8),
    /// The trailing Adler-32 did not match the computed checksum.
    BadChecksum { expected: u32, got: u32 },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadLength { expected, got } => {
                write!(f, "bad frame length: expected {expected}, got {got}")
            }
            DecodeError::BadMagic => write!(f, "bad magic bytes"),
            DecodeError::BadVersion(v) => write!(f, "unsupported protocol version {v}"),
            DecodeError::BadType(t) => write!(f, "unknown frame type {t}"),
            DecodeError::BadChecksum { expected, got } => {
                write!(f, "checksum mismatch: expected {expected:#010x}, got {got:#010x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

fn put_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

fn get_u16(buf: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([buf[at], buf[at + 1]])
}

fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// A prompt fragment painted onto the strip by the addon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFrame {
    /// Random id for the current UI session, so the companion can ignore stale frames.
    pub ui_session: u16,
    /// Monotonic request id within the UI session.
    pub request_id: u32,
    /// Zero-based index of this fragment.
    pub fragment_index: u16,
    /// Total number of fragments in the request.
    pub fragment_count: u16,
    /// Addon-defined flags (currently unused).
    pub flags: u8,
    /// Raw UTF-8 payload bytes (`len <= PROMPT_PAYLOAD_MAX`).
    pub payload: Vec<u8>,
}

impl PromptFrame {
    /// Encode to a 64-byte pixel frame.
    pub fn encode(&self) -> [u8; BYTES_PER_FRAME] {
        let mut buf = [0u8; BYTES_PER_FRAME];
        buf[0..2].copy_from_slice(&OUT_MAGIC);
        buf[2] = OUT_TYPE_PROMPT;
        buf[3] = PROTOCOL_VERSION;
        put_u16(&mut buf, 4, self.ui_session);
        put_u32(&mut buf, 6, self.request_id);
        put_u16(&mut buf, 10, self.fragment_index);
        put_u16(&mut buf, 12, self.fragment_count);
        let len = self.payload.len().min(PROMPT_PAYLOAD_MAX);
        buf[14] = len as u8;
        buf[15] = self.flags;
        buf[OUT_HEADER_LEN..OUT_HEADER_LEN + len].copy_from_slice(&self.payload[..len]);
        let sum = adler32(&buf[..60]);
        put_u32(&mut buf, 60, sum);
        buf
    }

    /// Decode from a 64-byte pixel frame.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        check_out_common(buf)?;
        if buf[2] != OUT_TYPE_PROMPT {
            return Err(DecodeError::BadType(buf[2]));
        }
        let len = (buf[14] as usize).min(PROMPT_PAYLOAD_MAX);
        Ok(PromptFrame {
            ui_session: get_u16(buf, 4),
            request_id: get_u32(buf, 6),
            fragment_index: get_u16(buf, 10),
            fragment_count: get_u16(buf, 12),
            flags: buf[15],
            payload: buf[OUT_HEADER_LEN..OUT_HEADER_LEN + len].to_vec(),
        })
    }
}

/// A scheduling frame: the addon tells the companion which font slot it is
/// about to load and which reply fragment it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFrame {
    pub ui_session: u16,
    pub request_id: u32,
    /// One-based fragment index the addon wants next.
    pub requested_fragment: u16,
    /// Font-bank slot the addon will load.
    pub slot: u16,
    /// Milliseconds from "now" until the addon loads the slot.
    pub deadline_ms: u32,
    /// Receiver state (see [`reply_state`]).
    pub state: u8,
    /// `true` while the addon is actively watching for a reply.
    pub active: bool,
    /// Highest slot the addon has finished attempting (for diagnostics/retry).
    pub last_attempted_slot: u16,
}

impl ControlFrame {
    /// Encode to a 64-byte pixel frame.
    pub fn encode(&self) -> [u8; BYTES_PER_FRAME] {
        let mut buf = [0u8; BYTES_PER_FRAME];
        buf[0..2].copy_from_slice(&OUT_MAGIC);
        buf[2] = OUT_TYPE_CONTROL;
        buf[3] = PROTOCOL_VERSION;
        put_u16(&mut buf, 4, self.ui_session);
        put_u32(&mut buf, 6, self.request_id);
        // Header fragment fields are unused for control frames.
        let p = OUT_HEADER_LEN;
        put_u16(&mut buf, p, self.requested_fragment);
        put_u16(&mut buf, p + 2, self.slot);
        put_u32(&mut buf, p + 4, self.deadline_ms);
        buf[p + 8] = self.state;
        buf[p + 9] = self.active as u8;
        put_u16(&mut buf, p + 10, self.last_attempted_slot);
        let sum = adler32(&buf[..60]);
        put_u32(&mut buf, 60, sum);
        buf
    }

    /// Decode from a 64-byte pixel frame.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        check_out_common(buf)?;
        if buf[2] != OUT_TYPE_CONTROL {
            return Err(DecodeError::BadType(buf[2]));
        }
        let p = OUT_HEADER_LEN;
        Ok(ControlFrame {
            ui_session: get_u16(buf, 4),
            request_id: get_u32(buf, 6),
            requested_fragment: get_u16(buf, p),
            slot: get_u16(buf, p + 2),
            deadline_ms: get_u32(buf, p + 4),
            state: buf[p + 8],
            active: buf[p + 9] != 0,
            last_attempted_slot: get_u16(buf, p + 10),
        })
    }
}

fn check_out_common(buf: &[u8]) -> Result<(), DecodeError> {
    if buf.len() != BYTES_PER_FRAME {
        return Err(DecodeError::BadLength {
            expected: BYTES_PER_FRAME,
            got: buf.len(),
        });
    }
    if buf[0..2] != OUT_MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if buf[3] != PROTOCOL_VERSION {
        return Err(DecodeError::BadVersion(buf[3]));
    }
    let expected = get_u32(buf, 60);
    let got = adler32(&buf[..60]);
    if expected != got {
        return Err(DecodeError::BadChecksum { expected, got });
    }
    Ok(())
}

/// A reply or status fragment encoded into font metrics by the companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyFrame {
    /// See [`reply_state`].
    pub state: u8,
    pub ui_session: u16,
    pub request_id: u32,
    /// One-based fragment index.
    pub fragment_index: u16,
    pub fragment_count: u16,
    /// Revision of the full reply; the addon discards partial assemblies when it changes.
    pub revision: u32,
    /// Font slot this packet was written into.
    pub slot: u16,
    /// Addon-defined flags (currently unused).
    pub flags: u8,
    /// Raw UTF-8 payload bytes (`len <= REPLY_PAYLOAD_MAX`).
    pub payload: Vec<u8>,
}

impl ReplyFrame {
    /// Encode to a 512-byte font payload.
    pub fn encode(&self) -> [u8; REPLY_FRAME_LEN] {
        let mut buf = [0u8; REPLY_FRAME_LEN];
        buf[0..2].copy_from_slice(&IN_MAGIC);
        buf[2] = PROTOCOL_VERSION;
        buf[3] = self.state;
        put_u16(&mut buf, 4, self.ui_session);
        put_u32(&mut buf, 6, self.request_id);
        put_u16(&mut buf, 10, self.fragment_index);
        put_u16(&mut buf, 12, self.fragment_count);
        let len = self.payload.len().min(REPLY_PAYLOAD_MAX);
        put_u16(&mut buf, 14, len as u16);
        put_u32(&mut buf, 16, self.revision);
        put_u16(&mut buf, 20, self.slot);
        buf[22] = self.flags;
        buf[IN_HEADER_LEN..IN_HEADER_LEN + len].copy_from_slice(&self.payload[..len]);
        let sum = adler32(&buf[..508]);
        put_u32(&mut buf, 508, sum);
        buf
    }

    /// Decode from a 512-byte font payload.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() != REPLY_FRAME_LEN {
            return Err(DecodeError::BadLength {
                expected: REPLY_FRAME_LEN,
                got: buf.len(),
            });
        }
        if buf[0..2] != IN_MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if buf[2] != PROTOCOL_VERSION {
            return Err(DecodeError::BadVersion(buf[2]));
        }
        let expected = get_u32(buf, 508);
        let got = adler32(&buf[..508]);
        if expected != got {
            return Err(DecodeError::BadChecksum { expected, got });
        }
        let len = (get_u16(buf, 14) as usize).min(REPLY_PAYLOAD_MAX);
        Ok(ReplyFrame {
            state: buf[3],
            ui_session: get_u16(buf, 4),
            request_id: get_u32(buf, 6),
            fragment_index: get_u16(buf, 10),
            fragment_count: get_u16(buf, 12),
            revision: get_u32(buf, 16),
            slot: get_u16(buf, 20),
            flags: buf[22],
            payload: buf[IN_HEADER_LEN..IN_HEADER_LEN + len].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_round_trip() {
        let f = PromptFrame {
            ui_session: 0xBEEF,
            request_id: 42,
            fragment_index: 1,
            fragment_count: 3,
            flags: 0,
            payload: b"look at my quest log".to_vec(),
        };
        let buf = f.encode();
        assert_eq!(buf.len(), BYTES_PER_FRAME);
        assert_eq!(PromptFrame::decode(&buf).unwrap(), f);
    }

    #[test]
    fn prompt_handles_max_payload() {
        let f = PromptFrame {
            ui_session: 1,
            request_id: 1,
            fragment_index: 0,
            fragment_count: 32,
            flags: 0,
            payload: vec![b'x'; PROMPT_PAYLOAD_MAX],
        };
        assert_eq!(PromptFrame::decode(&f.encode()).unwrap(), f);
    }

    #[test]
    fn prompt_handles_utf8() {
        let text = "Warcraft: Ærøskøbing 🐉".as_bytes().to_vec();
        let mut chunks = text.chunks(PROMPT_PAYLOAD_MAX);
        let first = chunks.next().unwrap().to_vec();
        let f = PromptFrame {
            ui_session: 7,
            request_id: 9,
            fragment_index: 0,
            fragment_count: 1,
            flags: 0,
            payload: first,
        };
        assert_eq!(PromptFrame::decode(&f.encode()).unwrap(), f);
    }

    #[test]
    fn control_round_trip() {
        let f = ControlFrame {
            ui_session: 3,
            request_id: 1234,
            requested_fragment: 2,
            slot: 77,
            deadline_ms: 5000,
            state: reply_state::STREAMING,
            active: true,
            last_attempted_slot: 76,
        };
        assert_eq!(ControlFrame::decode(&f.encode()).unwrap(), f);
    }

    #[test]
    fn reply_round_trip() {
        let f = ReplyFrame {
            state: reply_state::DONE,
            ui_session: 0x0102,
            request_id: 0xDEAD_BEEF,
            fragment_index: 1,
            fragment_count: 2,
            revision: 0x1234_5678,
            slot: 512,
            flags: 0,
            payload: b"Here is the answer you asked for.".to_vec(),
        };
        let buf = f.encode();
        assert_eq!(buf.len(), REPLY_FRAME_LEN);
        assert_eq!(ReplyFrame::decode(&buf).unwrap(), f);
    }

    #[test]
    fn detects_corruption() {
        let f = PromptFrame {
            ui_session: 1,
            request_id: 2,
            fragment_index: 0,
            fragment_count: 1,
            flags: 0,
            payload: b"hi".to_vec(),
        };
        let mut buf = f.encode();
        buf[25] ^= 0xFF;
        assert!(matches!(
            PromptFrame::decode(&buf),
            Err(DecodeError::BadChecksum { .. })
        ));
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        let f = ControlFrame {
            ui_session: 1,
            request_id: 1,
            requested_fragment: 1,
            slot: 1,
            deadline_ms: 1000,
            state: reply_state::WAITING,
            active: false,
            last_attempted_slot: 0,
        };
        let mut buf = f.encode();
        buf[0] = b'X';
        assert_eq!(ControlFrame::decode(&buf), Err(DecodeError::BadMagic));

        let mut buf = f.encode();
        buf[3] = 99;
        assert_eq!(ControlFrame::decode(&buf), Err(DecodeError::BadVersion(99)));
    }
}
