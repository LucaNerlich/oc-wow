//! Wire protocol shared with the `OCWow` Lua addon.
//!
//! See `docs/protocol.md` for the byte-level specification.

pub mod adler32;
pub mod frames;
pub mod pixel;

pub use adler32::adler32;
pub use frames::{
    ControlFrame, DecodeError, PromptFrame, ReplyFrame, OUT_MAGIC, IN_MAGIC, OUT_TYPE_CONTROL,
    OUT_TYPE_PROMPT, PROMPT_PAYLOAD_MAX, REPLY_PAYLOAD_MAX,
};
pub use pixel::{
    bytes_to_cells, cells_to_bytes, decode_strip, GrayImage, BYTES_PER_FRAME, CELL_COUNT,
    DEFAULT_CELL_PX, STRIP_COLS, STRIP_ROWS,
};
