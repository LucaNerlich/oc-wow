//! Mapping between the on-screen black/white cell strip and raw bytes.
//!
//! The strip is [`STRIP_ROWS`] rows of [`STRIP_COLS`] cells. Cell index is
//! row-major (`row * STRIP_COLS + col`). A white cell is a `1` bit, a black
//! cell is a `0` bit. Bits are packed MSB-first into [`BYTES_PER_FRAME`] bytes.

/// Cells per row of the strip.
pub const STRIP_COLS: usize = 128;
/// Rows in the strip.
pub const STRIP_ROWS: usize = 4;
/// Total cells (bits) per displayed frame.
pub const CELL_COUNT: usize = STRIP_COLS * STRIP_ROWS;
/// Bytes carried by one displayed frame.
pub const BYTES_PER_FRAME: usize = CELL_COUNT / 8;
/// Default cell edge length in screen pixels.
pub const DEFAULT_CELL_PX: u32 = 4;

/// Luminance at or above this value is treated as a white (`1`) cell.
pub const WHITE_THRESHOLD: u8 = 160;
/// Luminance at or below this value is treated as a black (`0`) cell.
pub const BLACK_THRESHOLD: u8 = 96;

/// Pack 512 cell bits (row-major, `true` = white) into 64 bytes, MSB-first.
///
/// Returns `None` when `cells` is not exactly [`CELL_COUNT`] long.
pub fn cells_to_bytes(cells: &[bool]) -> Option<[u8; BYTES_PER_FRAME]> {
    if cells.len() != CELL_COUNT {
        return None;
    }
    let mut out = [0u8; BYTES_PER_FRAME];
    for (i, &bit) in cells.iter().enumerate() {
        if bit {
            out[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    Some(out)
}

/// Unpack 64 bytes into 512 cell bits (row-major, `true` = white).
pub fn bytes_to_cells(bytes: &[u8; BYTES_PER_FRAME]) -> [bool; CELL_COUNT] {
    let mut cells = [false; CELL_COUNT];
    for (i, cell) in cells.iter_mut().enumerate() {
        *cell = (bytes[i / 8] >> (7 - (i % 8))) & 1 == 1;
    }
    cells
}

/// A single-channel (luminance) view over a screen-capture crop.
///
/// `origin` is the index of the top-left pixel and `stride` is the number of
/// bytes per row, so sub-views can be taken without copying.
pub struct GrayImage<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub origin: usize,
    pub data: &'a [u8],
}

impl<'a> GrayImage<'a> {
    /// Wrap a tightly packed image (`stride == width`, origin 0).
    pub fn new(width: u32, height: u32, data: &'a [u8]) -> Self {
        Self {
            width,
            height,
            stride: width,
            origin: 0,
            data,
        }
    }

    /// A sub-view of this image.
    pub fn sub(&self, x: u32, y: u32, width: u32, height: u32) -> GrayImage<'a> {
        GrayImage {
            width,
            height,
            stride: self.stride,
            origin: self.origin + y as usize * self.stride as usize + x as usize,
            data: self.data,
        }
    }

    /// Average luminance of a small window centred on `(x, y)`.
    pub fn sample(&self, x: u32, y: u32) -> u8 {
        let mut sum: u32 = 0;
        let mut count: u32 = 0;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let sx = x as i32 + dx;
                let sy = y as i32 + dy;
                if sx < 0 || sy < 0 || sx >= self.width as i32 || sy >= self.height as i32 {
                    continue;
                }
                let idx = self.origin + sy as usize * self.stride as usize + sx as usize;
                sum += self.data[idx] as u32;
                count += 1;
            }
        }
        if count == 0 {
            0
        } else {
            (sum / count) as u8
        }
    }
}

/// Sample the centre of every cell and decode one frame at a known cell size.
///
/// Returns `None` when the view is too small or any cell is ambiguous
/// (neither clearly black nor clearly white), so a misaligned or occluded
/// capture fails loudly instead of decoding garbage.
pub fn decode_strip(img: &GrayImage, cell_px: u32) -> Option<[u8; BYTES_PER_FRAME]> {
    if cell_px == 0 {
        return None;
    }
    let need_w = STRIP_COLS as u32 * cell_px;
    let need_h = STRIP_ROWS as u32 * cell_px;
    if img.width < need_w || img.height < need_h {
        return None;
    }

    let mut cells = [false; CELL_COUNT];
    for row in 0..STRIP_ROWS as u32 {
        for col in 0..STRIP_COLS as u32 {
            let cx = col * cell_px + cell_px / 2;
            let cy = row * cell_px + cell_px / 2;
            let lum = img.sample(cx, cy);
            let bit = if lum >= WHITE_THRESHOLD {
                true
            } else if lum <= BLACK_THRESHOLD {
                false
            } else {
                return None;
            };
            cells[row as usize * STRIP_COLS + col as usize] = bit;
        }
    }
    cells_to_bytes(&cells)
}

/// Search a captured region for the strip.
///
/// Tries plausible cell sizes and small pixel offsets, and validates each
/// candidate with the frame magic and checksum. Used by `ocw probe` to find
/// the exact crop for a given UI scale and monitor layout.
pub fn find_strip(img: &GrayImage) -> Option<(u32, u32, u32, [u8; BYTES_PER_FRAME])> {
    for cell in 2..=8u32 {
        let w = STRIP_COLS as u32 * cell;
        let h = STRIP_ROWS as u32 * cell;
        if w > img.width || h > img.height {
            continue;
        }
        let max_dx = (img.width - w).min(96);
        let max_dy = (img.height - h).min(96);
        for dy in 0..=max_dy {
            for dx in 0..=max_dx {
                let view = img.sub(dx, dy, w, h);
                if let Some(bytes) = decode_strip(&view, cell) {
                    if looks_like_frame(&bytes) {
                        return Some((cell, dx, dy, bytes));
                    }
                }
            }
        }
    }
    None
}

/// Cheap sanity check: valid magic, version and Adler-32.
pub fn is_valid_frame(bytes: &[u8; BYTES_PER_FRAME]) -> bool {
    looks_like_frame(bytes)
}

/// Cheap sanity check used by [`find_strip`]: valid magic plus a matching
/// Adler-32. Kept local to avoid a dependency cycle on `frames`.
fn looks_like_frame(bytes: &[u8; BYTES_PER_FRAME]) -> bool {
    if bytes[0..2] != *b"OC" || bytes[3] != 1 {
        return false;
    }
    let expected = u32::from_be_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]);
    expected == crate::protocol::adler32::adler32(&bytes[..60])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_bits() {
        let mut bytes = [0u8; BYTES_PER_FRAME];
        bytes[0] = 0b1010_1010;
        bytes[63] = 0b0000_0001;
        let cells = bytes_to_cells(&bytes);
        assert!(cells[0] && !cells[1]);
        assert_eq!(cells_to_bytes(&cells).unwrap(), bytes);
    }

    fn paint(payload: &[u8; BYTES_PER_FRAME], cell: u32, pad: u32) -> (Vec<u8>, u32, u32) {
        let w = STRIP_COLS as u32 * cell + pad * 2;
        let h = STRIP_ROWS as u32 * cell + pad * 2;
        let mut img = vec![0u8; (w * h) as usize];
        let cells = bytes_to_cells(payload);
        for row in 0..STRIP_ROWS as u32 {
            for col in 0..STRIP_COLS as u32 {
                if cells[row as usize * STRIP_COLS + col as usize] {
                    for dy in 0..cell {
                        for dx in 0..cell {
                            let x = pad + col * cell + dx;
                            let y = pad + row * cell + dy;
                            img[(y * w + x) as usize] = 255;
                        }
                    }
                }
            }
        }
        (img, w, h)
    }

    #[test]
    fn renders_and_decodes_a_frame() {
        let cell = 4u32;
        let mut payload = [0u8; BYTES_PER_FRAME];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        let (img, w, h) = paint(&payload, cell, 0);
        let gray = GrayImage::new(w, h, &img);
        assert_eq!(decode_strip(&gray, cell).unwrap(), payload);
    }

    #[test]
    fn finds_strip_with_padding_and_scale() {
        let cell = 6u32;
        let mut payload = [0u8; BYTES_PER_FRAME];
        // A frame whose magic and checksum are valid.
        payload[0] = b'O';
        payload[1] = b'C';
        payload[2] = 1; // prompt type
        payload[3] = 1; // version
        let sum = crate::protocol::adler32::adler32(&payload[..60]);
        payload[60..64].copy_from_slice(&sum.to_be_bytes());

        let (img, w, h) = paint(&payload, cell, 20);
        let gray = GrayImage::new(w, h, &img);
        let (found_cell, dx, dy, bytes) = find_strip(&gray).expect("strip found");
        assert_eq!(found_cell, cell);
        // Several offsets decode correctly; the search returns the first, which
        // may sit up to one cell before the true origin.
        assert!(dx <= 20 && 20 - dx < cell, "dx={dx}");
        assert!(dy <= 20 && 20 - dy < cell, "dy={dy}");
        assert_eq!(bytes, payload);
    }

    #[test]
    fn rejects_ambiguous_cells() {
        let cell = 4u32;
        let w = STRIP_COLS as u32 * cell;
        let h = STRIP_ROWS as u32 * cell;
        let img = vec![128u8; (w * h) as usize];
        let gray = GrayImage::new(w, h, &img);
        assert!(decode_strip(&gray, cell).is_none());
    }

    #[test]
    fn rejects_undersized_crop() {
        let img = vec![0u8; 16];
        let gray = GrayImage::new(4, 4, &img);
        assert!(decode_strip(&gray, 4).is_none());
    }
}
