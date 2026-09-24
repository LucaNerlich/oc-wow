//! The pixel-strip wire format (outbound: addon -> companion).
//!
//! The addon paints a message as a grid of coloured cells in the top-left of the
//! game window; the companion screen-captures that region and decodes it. This
//! replaces every earlier outbound channel: no chat log, no file reads, no
//! calibration guesswork beyond finding the window.
//!
//! Layout, following the same rules the reference addon measured on this client:
//!
//! * each cell is 3 bits, one bit per colour channel, fully on or off — pure
//!   primaries survive any gamma/contrast setting;
//! * cells are packed 3 bits at a time, MSB-first, into a byte stream;
//! * the byte stream is `[magic][id][len][payload…][Fletcher-16]`.

/// Marks the start of a frame: `0xC7 0x1A`.
pub const MAGIC: [u8; 2] = [0xC7, 0x1A];
/// Bits carried by one cell.
pub const CELL_BITS: usize = 3;
/// Cells per row of the strip.
pub const CELLS_PER_ROW: usize = 200;
/// Rows the addon may draw.
pub const MAX_ROWS: usize = 48;
/// Cell edge length in screen pixels.
pub const CELL_PX: u32 = 4;
/// Record separator inside a payload.
pub const RECORD_SEP: u8 = 0x1E;
/// Field separator inside a record.
pub const FIELD_SEP: u8 = 0x1F;

/// Header bytes: magic, id (2), length (2).
const HEADER: usize = 6;
/// Trailer bytes: Fletcher-16.
const TRAILER: usize = 2;
/// Most payload bytes that fit in a full strip.
pub const MAX_PAYLOAD: usize =
    (CELLS_PER_ROW * MAX_ROWS * CELL_BITS / 8) - HEADER - TRAILER;

/// Width of the strip in screen pixels when every cell is drawn.
pub const STRIP_WIDTH_PX: u32 = CELLS_PER_ROW as u32 * CELL_PX;
/// Height of a strip with `rows` rows, in screen pixels.
pub fn strip_height_px(rows: usize) -> u32 {
    rows as u32 * CELL_PX
}

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripError {
    /// The leading bytes were not the magic.
    BadMagic,
    /// The declared length does not fit the strip.
    BadLength(u16),
    /// The Fletcher-16 checksum did not match.
    BadChecksum,
    /// The captured image did not contain a whole frame.
    Truncated,
}

impl std::fmt::Display for StripError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StripError::BadMagic => write!(f, "magic"),
            StripError::BadLength(n) => write!(f, "length ({n})"),
            StripError::BadChecksum => write!(f, "checksum"),
            StripError::Truncated => write!(f, "truncated"),
        }
    }
}

/// Fletcher-16 over the id, length and payload, as the addon computes it.
pub fn fletcher16(data: &[u8]) -> (u8, u8) {
    let mut s1: u32 = 0;
    let mut s2: u32 = 0;
    for &byte in data {
        s1 = (s1 + byte as u32) % 255;
        s2 = (s2 + s1) % 255;
    }
    (s1 as u8, s2 as u8)
}

/// The colour of a cell: one bit per channel, red high, blue low.
pub fn cell_color(value: u8) -> (f32, f32, f32) {
    (
        if value & 0b100 != 0 { 1.0 } else { 0.0 },
        if value & 0b010 != 0 { 1.0 } else { 0.0 },
        if value & 0b001 != 0 { 1.0 } else { 0.0 },
    )
}

/// Pack bytes into 3-bit cell values.
pub fn bytes_to_cells(bytes: &[u8]) -> Vec<u8> {
    let mut cells = Vec::with_capacity(bytes.len() * 8 / CELL_BITS + 1);
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    for &byte in bytes {
        accumulator = (accumulator << 8) | byte as u32;
        bits += 8;
        while bits >= CELL_BITS as u32 {
            bits -= CELL_BITS as u32;
            cells.push(((accumulator >> bits) & 0b111) as u8);
        }
    }
    if bits > 0 {
        cells.push(((accumulator << (CELL_BITS as u32 - bits)) & 0b111) as u8);
    }
    cells
}

/// Unpack 3-bit cell values into bytes.
pub fn cells_to_bytes(cells: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(cells.len() * CELL_BITS / 8);
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    for &cell in cells {
        accumulator = (accumulator << CELL_BITS) | (cell & 0b111) as u32;
        bits += CELL_BITS as u32;
        if bits >= 8 {
            bits -= 8;
            bytes.push(((accumulator >> bits) & 0xFF) as u8);
        }
    }
    bytes
}

/// Build the full frame for `payload` under message id `id`.
pub fn encode_frame(id: u16, payload: &[u8]) -> Vec<u8> {
    let len = payload.len().min(MAX_PAYLOAD) as u16;
    let mut bytes = Vec::with_capacity(HEADER + len as usize + TRAILER);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&payload[..len as usize]);
    let (s1, s2) = fletcher16(&bytes[2..]);
    bytes.push(s1);
    bytes.push(s2);
    bytes
}

/// Validate a byte stream and return the message id and payload.
pub fn decode_frame(bytes: &[u8]) -> Result<(u16, Vec<u8>), StripError> {
    if bytes.len() < HEADER + TRAILER {
        return Err(StripError::Truncated);
    }
    if bytes[0..2] != MAGIC {
        return Err(StripError::BadMagic);
    }
    let id = u16::from_be_bytes([bytes[2], bytes[3]]);
    let len = u16::from_be_bytes([bytes[4], bytes[5]]);
    if len as usize > MAX_PAYLOAD {
        return Err(StripError::BadLength(len));
    }
    let end = HEADER + len as usize;
    if bytes.len() < end + TRAILER {
        return Err(StripError::Truncated);
    }
    let (s1, s2) = fletcher16(&bytes[2..end]);
    if bytes[end] != s1 || bytes[end + 1] != s2 {
        return Err(StripError::BadChecksum);
    }
    Ok((id, bytes[HEADER..end].to_vec()))
}

/// Split a decoded payload into records and fields.
pub fn parse_records(payload: &[u8]) -> Vec<Vec<Vec<u8>>> {
    payload
        .split(|&b| b == RECORD_SEP)
        .filter(|record| !record.is_empty())
        .map(|record| record.split(|&b| b == FIELD_SEP).map(|f| f.to_vec()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_round_trip() {
        let bytes: Vec<u8> = (0..64u16).map(|i| (i * 37 % 251) as u8).collect();
        let cells = bytes_to_cells(&bytes);
        assert!(cells.iter().all(|&c| c < 8), "cells are 3 bits");
        // Trailing padding bits are expected; compare the leading bytes.
        let recovered = cells_to_bytes(&cells);
        assert_eq!(&recovered[..bytes.len()], &bytes[..]);
    }

    #[test]
    fn frame_round_trips() {
        let payload = b"hello from the addon";
        let cells = bytes_to_cells(&encode_frame(4242, payload));
        let (id, decoded) = decode_frame(&cells_to_bytes(&cells)).unwrap();
        assert_eq!(id, 4242);
        assert_eq!(decoded, payload);
    }

    #[test]
    fn magic_is_detected() {
        let mut bytes = encode_frame(1, b"x");
        bytes[0] = 0;
        assert_eq!(decode_frame(&bytes), Err(StripError::BadMagic));
    }

    #[test]
    fn checksum_is_detected() {
        let mut bytes = encode_frame(1, b"some payload");
        let end = HEADER + 12;
        bytes[end] ^= 0xFF;
        assert_eq!(decode_frame(&bytes), Err(StripError::BadChecksum));
    }

    #[test]
    fn rejects_absurd_lengths() {
        let mut bytes = encode_frame(1, b"x");
        bytes[4] = 0xFF;
        bytes[5] = 0xFF;
        assert!(matches!(decode_frame(&bytes), Err(StripError::BadLength(_))));
    }

    #[test]
    fn cell_colours_are_primaries() {
        assert_eq!(cell_color(0b111), (1.0, 1.0, 1.0));
        assert_eq!(cell_color(0b100), (1.0, 0.0, 0.0));
        assert_eq!(cell_color(0b001), (0.0, 0.0, 1.0));
        assert_eq!(cell_color(0b000), (0.0, 0.0, 0.0));
    }

    #[test]
    fn records_split_on_separators() {
        let payload = b"a\x1fb\x1ec\x1fd";
        let records = parse_records(payload);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], vec![b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(records[1], vec![b"c".to_vec(), b"d".to_vec()]);
    }

    #[test]
    fn magic_packs_into_the_expected_cells() {
        // The first cells of every frame are fixed, which is what calibration
        // searches for on screen.
        let cells = bytes_to_cells(&MAGIC);
        assert_eq!(cells, vec![0b110, 0b001, 0b110, 0b001, 0b101, 0b000]);
    }
}
