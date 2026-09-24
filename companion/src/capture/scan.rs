//! Locating and reading the pixel strip inside a captured image.

use crate::capture::png::RgbImage;
use crate::protocol::strip::{bytes_to_cells, CELLS_PER_ROW, MAX_ROWS, MAGIC};

/// Where the strip starts in a captured image, and how big its cells are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripLocation {
    pub x: u32,
    pub y: u32,
    pub cell_px: u32,
}

impl StripLocation {
    /// Width of a full strip at this cell size.
    pub fn width(&self) -> u32 {
        CELLS_PER_ROW as u32 * self.cell_px
    }

    /// Height of `rows` rows at this cell size.
    pub fn height(&self, rows: usize) -> u32 {
        rows as u32 * self.cell_px
    }
}

/// Classify one pixel as a 3-bit cell value: one bit per channel, mid-grey
/// counts as on.
fn cell_value(r: u8, g: u8, b: u8) -> u8 {
    let mut value = 0;
    if r >= 128 {
        value |= 0b100;
    }
    if g >= 128 {
        value |= 0b010;
    }
    if b >= 128 {
        value |= 0b001;
    }
    value
}

fn sample(image: &RgbImage, x: u32, y: u32) -> u8 {
    let (r, g, b) = image.pixel(x, y);
    cell_value(r, g, b)
}

/// Search a captured image for the start of a strip.
///
/// Every frame begins with the same two magic bytes, so the first six cells are
/// a fixed colour sequence. The scan rejects on two of them before checking the
/// rest, and then **validates by decoding the frame**, which is what stops a
/// wrong cell size from producing a false match.
pub fn find_strip(image: &RgbImage) -> Option<StripLocation> {
    let magic = bytes_to_cells(&MAGIC);
    if magic.len() < 5 {
        return None;
    }

    // Four pixels is what the addon's scale trick produces; try it first.
    for cell_px in [4u32, 3, 5, 2, 6, 7, 8] {
        let half = cell_px / 2;
        let width = CELLS_PER_ROW as u32 * cell_px;
        // Only one row has to fit: a short message draws a short strip.
        if image.width < width || image.height < cell_px {
            continue;
        }

        for y in 0..=(image.height - cell_px) {
            let cy = y + half;
            for x in 0..=(image.width - magic.len() as u32 * cell_px) {
                if sample(image, x + half, cy) != magic[0] {
                    continue;
                }
                if sample(image, x + cell_px + half, cy) != magic[1] {
                    continue;
                }
                if sample(image, x + 4 * cell_px + half, cy) != magic[4] {
                    continue;
                }
                let mut matches = true;
                for (index, &want) in magic.iter().enumerate() {
                    if sample(image, x + index as u32 * cell_px + half, cy) != want {
                        matches = false;
                        break;
                    }
                }
                if !matches {
                    continue;
                }
                let location = StripLocation { x, y, cell_px };
                if decodes(image, &location) {
                    return Some(location);
                }
            }
        }
    }
    None
}

/// Whether a candidate location holds a whole, checksum-valid frame.
fn decodes(image: &RgbImage, location: &StripLocation) -> bool {
    let cells = sample_strip(image, location);
    let bytes = crate::protocol::strip::cells_to_bytes(&cells);
    crate::protocol::strip::decode_frame(&bytes).is_ok()
}

/// Read every cell of the strip into a flat, row-major list of 3-bit values.
pub fn sample_strip(image: &RgbImage, location: &StripLocation) -> Vec<u8> {
    let half = location.cell_px / 2;
    let rows = ((image.height.saturating_sub(location.y)) / location.cell_px) as usize;
    let rows = rows.clamp(1, MAX_ROWS);

    let mut cells = Vec::with_capacity(CELLS_PER_ROW * rows);
    for row in 0..rows {
        let y = location.y + row as u32 * location.cell_px + half;
        for column in 0..CELLS_PER_ROW {
            let x = location.x + column as u32 * location.cell_px + half;
            cells.push(sample(image, x, y));
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::strip::{bytes_to_cells, cell_color, encode_frame};

    /// Paint a frame into a synthetic screenshot at a given cell size and offset.
    fn paint(frame: &[u8], cell_px: u32, offset: (u32, u32), pad: u32) -> RgbImage {
        let cells = bytes_to_cells(frame);
        let rows = (cells.len() + CELLS_PER_ROW - 1) / CELLS_PER_ROW;
        let width = CELLS_PER_ROW as u32 * cell_px + pad * 2 + offset.0;
        let height = rows as u32 * cell_px + pad * 2 + offset.1;
        let mut data = vec![0u8; (width * height * 3) as usize];

        for (index, &value) in cells.iter().enumerate() {
            let (r, g, b) = cell_color(value);
            let (cr, cg, cb) = (
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            );
            let column = index % CELLS_PER_ROW;
            let row = index / CELLS_PER_ROW;
            for dy in 0..cell_px {
                for dx in 0..cell_px {
                    let x = pad + offset.0 + column as u32 * cell_px + dx;
                    let y = pad + offset.1 + row as u32 * cell_px + dy;
                    let at = ((y * width + x) * 3) as usize;
                    data[at] = cr;
                    data[at + 1] = cg;
                    data[at + 2] = cb;
                }
            }
        }
        RgbImage {
            width,
            height,
            data,
        }
    }

    #[test]
    fn finds_and_reads_a_frame() {
        let payload = b"hello from the addon";
        let frame = encode_frame(4242, payload);
        let image = paint(&frame, 4, (37, 19), 8);

        let location = find_strip(&image).expect("strip found");
        assert_eq!(location.cell_px, 4);
        // Sampling the centre of each cell leaves a little slack in the origin.
        assert!((location.x as i32 - (8 + 37) as i32).abs() <= 4, "x={}", location.x);
        assert!((location.y as i32 - (8 + 19) as i32).abs() <= 4, "y={}", location.y);

        let cells = sample_strip(&image, &location);
        let bytes = crate::protocol::strip::cells_to_bytes(&cells);
        let (id, decoded) = crate::protocol::strip::decode_frame(&bytes).unwrap();
        assert_eq!(id, 4242);
        assert_eq!(decoded, payload);
    }

    #[test]
    fn finds_a_frame_at_other_cell_sizes() {
        let frame = encode_frame(7, b"x");
        for cell_px in [2u32, 3, 5, 6, 8] {
            let image = paint(&frame, cell_px, (5, 5), 0);
            let location = find_strip(&image).expect("strip found");
            assert_eq!(location.cell_px, cell_px);
            let cells = sample_strip(&image, &location);
            let (id, _) = crate::protocol::strip::decode_frame(
                &crate::protocol::strip::cells_to_bytes(&cells),
            )
            .unwrap();
            assert_eq!(id, 7);
        }
    }

    #[test]
    fn returns_none_on_blank_images() {
        let image = RgbImage {
            width: 1200,
            height: 200,
            data: vec![0; 1200 * 200 * 3],
        };
        assert!(find_strip(&image).is_none());
    }
}
