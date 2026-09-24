//! A tiny PNG decoder, sufficient for the 8-bit RGB/RGBA screenshots produced
//! by `screencapture`, `grim`, ImageMagick, and friends.
//!
//! Only the subset needed for screen capture is supported: 8-bit depth,
//! non-interlaced, colour types 0 (grey), 2 (RGB), 4 (grey+alpha) and 6 (RGBA).

use anyhow::{bail, Result};

use crate::protocol::pixel::GrayImage;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A decoded screenshot as 8-bit luminance.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub gray: Vec<u8>,
}

impl DecodedImage {
    /// Borrow as a [`GrayImage`] view.
    pub fn as_gray(&self) -> GrayImage<'_> {
        GrayImage::new(self.width, self.height, &self.gray)
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    // Integer Rec. 601 luma.
    ((299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000) as u8
}

/// Decode a PNG file into 8-bit luminance.
pub fn decode_png(bytes: &[u8]) -> Result<DecodedImage> {
    if bytes.len() < 8 || bytes[..8] != PNG_SIGNATURE {
        bail!("not a PNG file");
    }

    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_ihdr = false;

    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let ctype = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start + len;
        if data_end + 4 > bytes.len() {
            bail!("truncated PNG chunk");
        }
        let data = &bytes[data_start..data_end];

        match ctype {
            b"IHDR" => {
                if len < 13 {
                    bail!("malformed IHDR");
                }
                width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                bit_depth = data[8];
                color_type = data[9];
                interlace = data[12];
                seen_ihdr = true;
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos = data_end + 4;
    }

    if !seen_ihdr {
        bail!("PNG has no IHDR chunk");
    }
    if bit_depth != 8 {
        bail!("unsupported PNG bit depth {bit_depth} (only 8-bit is supported)");
    }
    if interlace != 0 {
        bail!("interlaced PNGs are not supported");
    }

    let channels: usize = match color_type {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        other => bail!("unsupported PNG colour type {other}"),
    };

    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .map_err(|e| anyhow::anyhow!("zlib inflate failed: {e:?}"))?;

    let row_len = width as usize * channels;
    let needed = (row_len + 1) * height as usize;
    if raw.len() < needed {
        bail!("PNG pixel data shorter than expected");
    }

    let bpp = channels;
    let mut unfiltered = vec![0u8; row_len * height as usize];
    let mut prev = vec![0u8; row_len];

    for y in 0..height as usize {
        let base = y * (row_len + 1);
        let filter = raw[base];
        let src = &raw[base + 1..base + 1 + row_len];
        let mut cur = vec![0u8; row_len];
        for x in 0..row_len {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = prev[x];
            let c = if x >= bpp { prev[x - bpp] } else { 0 };
            cur[x] = match filter {
                0 => src[x],
                1 => src[x].wrapping_add(a),
                2 => src[x].wrapping_add(b),
                3 => src[x].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => src[x].wrapping_add(paeth(a, b, c)),
                other => bail!("unsupported PNG filter {other}"),
            };
        }
        unfiltered[y * row_len..(y + 1) * row_len].copy_from_slice(&cur);
        prev = cur;
    }

    let pixels = width as usize * height as usize;
    let mut gray = vec![0u8; pixels];
    for (i, g) in gray.iter_mut().enumerate() {
        *g = match color_type {
            0 => unfiltered[i],
            2 => luma(unfiltered[i * 3], unfiltered[i * 3 + 1], unfiltered[i * 3 + 2]),
            4 => unfiltered[i * 2],
            6 => luma(
                unfiltered[i * 4],
                unfiltered[i * 4 + 1],
                unfiltered[i * 4 + 2],
            ),
            _ => unreachable!(),
        };
    }

    Ok(DecodedImage {
        width,
        height,
        gray,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC ignored by the decoder
        out
    }

    fn encode_rgb(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // depth 8, truecolor, no interlace

        let mut raw = Vec::new();
        for y in 0..height as usize {
            raw.push(0); // filter: none
            raw.extend_from_slice(&pixels[y * width as usize * 3..(y + 1) * width as usize * 3]);
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);

        let mut png = Vec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"IDAT", &compressed));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn decodes_black_and_white() {
        let pixels = vec![
            0, 0, 0, 255, 255, 255, // row 0: black, white
            255, 255, 255, 0, 0, 0, // row 1: white, black
        ];
        let png = encode_rgb(2, 2, &pixels);
        let img = decode_png(&png).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.gray, vec![0, 255, 255, 0]);
    }

    #[test]
    fn rejects_non_png() {
        assert!(decode_png(b"not a png at all").is_err());
    }
}
