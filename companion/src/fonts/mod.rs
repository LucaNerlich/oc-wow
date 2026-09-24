//! TrueType generation and font-bank management for the inbound channel.

pub mod bank;
pub mod ttf;

pub use bank::{slot_filename, Bank, InstallReport, DEFAULT_SLOTS};
pub use ttf::{
    advance_for_byte, baseline_advances, build_baseline_font, build_font, build_reply_font,
    codepoint_for_data, decode_byte, read_advances, ADVANCE_BASE, ADVANCE_STEP, CODEPOINT_CAL_HIGH,
    CODEPOINT_CAL_LOW, CODEPOINT_DATA_BASE, CODEPOINT_TRAIL, DATA_GLYPHS, GLYPH_CAL_HIGH,
    GLYPH_CAL_LOW, GLYPH_DATA_BASE, GLYPH_TRAIL, NUM_GLYPHS, UPEM,
};
