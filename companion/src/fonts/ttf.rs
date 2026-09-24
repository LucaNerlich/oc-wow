//! A minimal TrueType font writer used for the companion -> addon channel.
//!
//! The font maps:
//!
//! | Codepoint      | Glyph | Meaning                                   |
//! |----------------|-------|-------------------------------------------|
//! | `U+007E` (`~`) | 1     | trailing glyph, cancels out of measurements |
//! | `U+E000..=U+E1FF` | 2..513 | data glyph *i* encodes reply byte *i*  |
//! | `U+E200`       | 514   | calibration glyph for byte 0              |
//! | `U+E201`       | 515   | calibration glyph for byte 255            |
//!
//! A data glyph for byte `b` has advance width `(16 + b) * 16` font units.
//! Glyphs carry only a **tiny** box outline, never a large one. Two hard-won
//! constraints from testing against the client:
//!
//! * A large outline (the byte value scaled into ink) overflows the client's
//!   font atlas when measuring, crashing it (`GxuFontMiscClasses.cpp`).
//! * A glyph with *no* outline is treated as missing and substituted with a
//!   fallback, which destroys the advance measurement.
//!
//! So every glyph gets the same small box; only the advance carries data.
//!
//! The addon recovers a byte with:
//!
//! ```text
//! low  = width(U+E200 .. "~")
//! high = width(U+E201 .. "~")
//! read = width(U+E000+i .. "~")
//! byte = round((read - low) * 255 / (high - low))
//! ```

/// Font units per em.
pub const UPEM: u16 = 1024;
/// Number of data glyphs (one per reply byte position).
pub const DATA_GLYPHS: usize = 512;
/// Total glyph count including `.notdef`, trailing and calibration glyphs.
pub const NUM_GLYPHS: u16 = (DATA_GLYPHS + 4) as u16;
/// Glyph id of the trailing glyph.
pub const GLYPH_TRAIL: u16 = 1;
/// Glyph id of the first data glyph.
pub const GLYPH_DATA_BASE: u16 = 2;
/// Glyph id of the byte-0 calibration glyph.
pub const GLYPH_CAL_LOW: u16 = 514;
/// Glyph id of the byte-255 calibration glyph.
pub const GLYPH_CAL_HIGH: u16 = 515;

/// Codepoint of the trailing glyph.
pub const CODEPOINT_TRAIL: u32 = 0x007E;
/// Codepoint of the first data glyph.
pub const CODEPOINT_DATA_BASE: u32 = 0xE000;
/// Codepoint of the byte-0 calibration glyph.
pub const CODEPOINT_CAL_LOW: u32 = 0xE200;
/// Codepoint of the byte-255 calibration glyph.
pub const CODEPOINT_CAL_HIGH: u32 = 0xE201;

/// Advance width encoding byte `0`.
pub const ADVANCE_BASE: u16 = 16 * 16;
/// Advance width step per byte value.
pub const ADVANCE_STEP: u16 = 16;

/// Advance width for a data byte.
pub fn advance_for_byte(b: u8) -> u16 {
    ADVANCE_BASE + ADVANCE_STEP * b as u16
}

/// Codepoint for data byte position `i`.
pub fn codepoint_for_data(i: usize) -> u32 {
    CODEPOINT_DATA_BASE + i as u32
}

/// Ink box size for every glyph, in font units. At the measurement size this is
/// only a few pixels square, which keeps the client's font atlas happy while
/// still giving the glyph a real outline.
const GLYPH_BOX: i16 = 64;
/// Serialised size of one glyph: 1 contour, 4 points, no hinting.
const GLYPH_LEN: usize = 34;

// ---------------------------------------------------------------------------
// Byte writer helpers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Buf(Vec<u8>);

impl Buf {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// Pad `v` with zeros until its length is a multiple of 4.
fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// OpenType table checksum: sum of big-endian u32 words, zero-padded.
fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        let mut word: u32 = 0;
        for k in 0..4 {
            let b = *data.get(i + k).unwrap_or(&0);
            word = (word << 8) | b as u32;
        }
        sum = sum.wrapping_add(word);
        i += 4;
    }
    sum
}

// ---------------------------------------------------------------------------
// Table builders
// ---------------------------------------------------------------------------

fn build_glyf(advances: &[u16]) -> Vec<u8> {
    // Every glyph is the same tiny box; the advance widths in `hmtx` carry the
    // data. See the module docs for why it cannot be empty and cannot be large.
    let mut out = Vec::with_capacity(advances.len() * GLYPH_LEN);
    for _ in advances {
        out.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
        out.extend_from_slice(&0i16.to_be_bytes()); // xMin
        out.extend_from_slice(&0i16.to_be_bytes()); // yMin
        out.extend_from_slice(&GLYPH_BOX.to_be_bytes()); // xMax
        out.extend_from_slice(&GLYPH_BOX.to_be_bytes()); // yMax

        out.extend_from_slice(&3u16.to_be_bytes()); // endPtsOfContours[0]
        out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        out.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]); // on-curve flags

        // x deltas: 0, +box, 0, -box
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&GLYPH_BOX.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&(-GLYPH_BOX).to_be_bytes());

        // y deltas: 0, 0, +box, 0
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
        out.extend_from_slice(&GLYPH_BOX.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    out
}

fn build_loca(glyf_len: usize, num_glyphs: u16) -> Vec<u8> {
    let glyph_len = glyf_len / num_glyphs as usize;
    let mut out = Vec::with_capacity((num_glyphs as usize + 1) * 2);
    for i in 0..=num_glyphs as usize {
        // Short format stores offset / 2.
        out.extend_from_slice(&((i * glyph_len / 2) as u16).to_be_bytes());
    }
    out
}

fn build_hmtx(advances: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(advances.len() * 4);
    for &advance in advances {
        out.extend_from_slice(&advance.to_be_bytes()); // advanceWidth
        out.extend_from_slice(&0i16.to_be_bytes()); // leftSideBearing
    }
    out
}

fn build_hhea(advance_max: u16) -> Vec<u8> {
    let mut b = Buf::default();
    b.u32(0x0001_0000); // version
    b.i16(UPEM as i16); // ascender
    b.i16(-(UPEM as i16) / 4); // descender
    b.i16(0); // lineGap
    b.u16(advance_max); // advanceWidthMax
    b.i16(0); // minLeftSideBearing
    b.i16(0); // minRightSideBearing
    b.i16(GLYPH_BOX); // xMaxExtent (the tiny box every glyph shares)
    b.i16(1); // caretSlopeRise
    b.i16(0); // caretSlopeRun
    b.i16(0); // caretOffset
    b.i16(0);
    b.i16(0);
    b.i16(0);
    b.i16(0); // reserved[4]
    b.i16(0); // metricDataFormat
    b.u16(NUM_GLYPHS); // numberOfHMetrics
    b.0
}

fn build_maxp() -> Vec<u8> {
    let mut b = Buf::default();
    b.u32(0x0001_0000); // version 1.0
    b.u16(NUM_GLYPHS);
    b.u16(4); // maxPoints
    b.u16(1); // maxContours
    b.u16(0); // maxCompositePoints
    b.u16(0); // maxCompositeContours
    b.u16(2); // maxZones
    b.u16(0); // maxTwilightPoints
    b.u16(0); // maxStorage
    b.u16(0); // maxFunctionDefs
    b.u16(0); // maxInstructionDefs
    b.u16(0); // maxStackElements
    b.u16(0); // maxSizeOfInstructions
    b.u16(0); // maxComponentElements
    b.u16(0); // maxComponentDepth
    b.0
}

fn build_head(index_to_loc_format: i16) -> Vec<u8> {
    let mut b = Buf::default();
    b.u32(0x0001_0000); // version
    b.u32(0x0001_0000); // fontRevision
    b.u32(0); // checkSumAdjustment (patched after assembly)
    b.u32(0x5F0F_3CF5); // magicNumber
    b.u16(0x0003); // flags
    b.u16(UPEM); // unitsPerEm
    b.i64(0); // created
    b.i64(0); // modified
    b.i16(0); // xMin
    b.i16(0); // yMin
    b.i16(GLYPH_BOX); // xMax (the tiny box every glyph shares)
    b.i16(GLYPH_BOX); // yMax
    b.u16(0); // macStyle
    b.u16(8); // lowestRecPPEM
    b.i16(2); // fontDirectionHint
    b.i16(index_to_loc_format); // indexToLocFormat
    b.i16(0); // glyphDataFormat
    b.0
}

fn build_os2() -> Vec<u8> {
    let mut b = Buf::default();
    b.u16(4); // version
    b.i16(ADVANCE_BASE as i16); // xAvgCharWidth
    b.u16(400); // usWeightClass
    b.u16(5); // usWidthClass
    b.u16(0); // fsType
    for _ in 0..10 {
        b.i16(0); // subscript/superscript/strikeout metrics
    }
    b.i16(0); // sFamilyClass
    b.bytes(&[0u8; 10]); // panose
    for _ in 0..4 {
        b.u32(0); // ulUnicodeRange
    }
    b.bytes(b"OCW "); // achVendID
    b.u16(0x0040); // fsSelection: REGULAR
    b.u16(CODEPOINT_TRAIL as u16); // usFirstCharIndex
    b.u16(CODEPOINT_CAL_HIGH as u16); // usLastCharIndex
    b.i16(UPEM as i16); // sTypoAscender
    b.i16(-(UPEM as i16) / 4); // sTypoDescender
    b.i16(0); // sTypoLineGap
    b.u16(UPEM); // usWinAscent
    b.u16(UPEM / 4); // usWinDescent
    b.u32(1); // ulCodePageRange1
    b.u32(0); // ulCodePageRange2
    b.i16(0); // sxHeight
    b.i16(0); // sCapHeight
    b.u16(0); // usDefaultChar
    b.u16(0x20); // usBreakChar
    b.u16(0); // usMaxContext
    b.0
}

fn build_post() -> Vec<u8> {
    let mut b = Buf::default();
    b.u32(0x0003_0000); // version 3.0 (no glyph names)
    b.u32(0); // italicAngle
    b.i16(-100); // underlinePosition
    b.i16(50); // underlineThickness
    b.u32(0); // isFixedPitch
    b.u32(0); // minMemType42
    b.u32(0); // maxMemType42
    b.u32(0); // minMemType1
    b.u32(0); // maxMemType1
    b.0
}

fn build_name() -> Vec<u8> {
    let names: [(&str, &str); 6] = [
        ("1", "OCWow Transport"),
        ("2", "Regular"),
        ("3", "OCWow Transport; ocw"),
        ("4", "OCWow Transport"),
        ("5", "Version 1.0"),
        ("6", "OCWowTransport"),
    ];

    let mut records = Vec::new();
    let mut strings = Vec::new();
    for (id, value) in names {
        let offset = strings.len();
        for unit in value.encode_utf16() {
            strings.extend_from_slice(&unit.to_be_bytes());
        }
        records.push((id.parse::<u16>().unwrap(), offset, value.encode_utf16().count()));
    }

    let count = records.len() as u16;
    let mut b = Buf::default();
    b.u16(0); // format
    b.u16(count);
    b.u16(6 + count * 12); // stringOffset
    for (id, offset, len) in &records {
        b.u16(3); // platformID: Windows
        b.u16(1); // encodingID: UCS-2
        b.u16(0x0409); // languageID: en-US
        b.u16(*id);
        b.u16(*len as u16 * 2);
        b.u16(*offset as u16);
    }
    b.bytes(&strings);
    b.0
}

/// Build a `cmap` with three format-4 segments.
fn build_cmap() -> Vec<u8> {
    // Segment 0: '~' -> glyph 1. Segment 1: U+E000..=U+E201 -> glyphs 2..=515.
    // Segment 2: 0xFFFF terminator.
    let seg_count: u16 = 3;
    let end_codes: [u16; 3] = [
        CODEPOINT_TRAIL as u16,
        CODEPOINT_CAL_HIGH as u16,
        0xFFFF,
    ];
    let start_codes: [u16; 3] = [
        CODEPOINT_TRAIL as u16,
        CODEPOINT_DATA_BASE as u16,
        0xFFFF,
    ];
    let id_deltas: [i16; 3] = [
        (GLYPH_TRAIL as i32 - CODEPOINT_TRAIL as i32) as i16,
        (GLYPH_DATA_BASE as i32 - CODEPOINT_DATA_BASE as i32) as i16,
        1,
    ];

    let mut sub = Buf::default();
    sub.u16(4); // format
    let length = 16 + seg_count * 8;
    sub.u16(length); // length
    sub.u16(0); // language
    sub.u16(seg_count * 2); // segCountX2

    let entry_selector = (seg_count as f32).log2().floor() as u16;
    let search_range = 2 * (1u16 << entry_selector);
    sub.u16(search_range);
    sub.u16(entry_selector);
    sub.u16(seg_count * 2 - search_range); // rangeShift

    for &e in &end_codes {
        sub.u16(e);
    }
    sub.u16(0); // reservedPad
    for &s in &start_codes {
        sub.u16(s);
    }
    for &d in &id_deltas {
        sub.i16(d);
    }
    for _ in 0..seg_count {
        sub.u16(0); // idRangeOffset
    }

    let mut b = Buf::default();
    b.u16(0); // version
    b.u16(1); // numTables
    b.u16(3); // platformID: Windows
    b.u16(1); // encodingID: Unicode BMP
    b.u32(12); // offset to subtable
    b.bytes(&sub.0);
    b.0
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Build a complete TrueType font from per-glyph advance widths.
///
/// `advances` must contain exactly [`NUM_GLYPHS`] entries.
pub fn build_font(advances: &[u16]) -> Vec<u8> {
    assert_eq!(advances.len(), NUM_GLYPHS as usize, "wrong glyph count");

    let glyf = build_glyf(advances);
    let loca = build_loca(glyf.len(), NUM_GLYPHS);
    let hmtx = build_hmtx(advances);
    let advance_max = *advances.iter().max().unwrap();

    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"OS/2", build_os2()),
        (*b"cmap", build_cmap()),
        (*b"glyf", glyf),
        (*b"head", build_head(0)),
        (*b"hhea", build_hhea(advance_max)),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", build_maxp()),
        (*b"name", build_name()),
        (*b"post", build_post()),
    ];
    tables.sort_by(|a, b| a.0.cmp(&b.0));

    let num_tables = tables.len() as u16;
    let entry_selector = (num_tables as f32).log2().floor() as u16;
    let search_range = 16 * (1u16 << entry_selector);

    let header_len = 12 + num_tables as usize * 16;
    let mut offset = header_len as u32;

    let mut directory: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();

    for (tag, data) in &tables {
        let mut padded = data.clone();
        pad4(&mut padded);

        directory.extend_from_slice(tag);
        directory.extend_from_slice(&checksum(data).to_be_bytes());
        directory.extend_from_slice(&offset.to_be_bytes());
        directory.extend_from_slice(&(data.len() as u32).to_be_bytes());

        body.extend_from_slice(&padded);
        offset += padded.len() as u32;
    }

    let mut font = Buf::default();
    font.u32(0x0001_0000); // sfntVersion
    font.u16(num_tables);
    font.u16(search_range);
    font.u16(entry_selector);
    font.u16(num_tables * 16 - search_range);
    font.bytes(&directory);
    font.bytes(&body);

    let mut font = font.0;

    // Patch head.checkSumAdjustment.
    let head_offset = {
        let mut found = None;
        for (tag, _, off, _) in parse_directory(&font) {
            if &tag == b"head" {
                found = Some(off as usize);
            }
        }
        found.expect("head table present")
    };
    let whole = checksum(&font);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(whole);
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());

    font
}

fn parse_directory(font: &[u8]) -> Vec<([u8; 4], u32, u32, u32)> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut out = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = 12 + i * 16;
        let tag = [font[base], font[base + 1], font[base + 2], font[base + 3]];
        let sum = u32::from_be_bytes([
            font[base + 4],
            font[base + 5],
            font[base + 6],
            font[base + 7],
        ]);
        let off = u32::from_be_bytes([
            font[base + 8],
            font[base + 9],
            font[base + 10],
            font[base + 11],
        ]);
        let len = u32::from_be_bytes([
            font[base + 12],
            font[base + 13],
            font[base + 14],
            font[base + 15],
        ]);
        out.push((tag, sum, off, len));
    }
    out
}

/// Advance widths for the baseline (all-zero) font.
pub fn baseline_advances() -> Vec<u16> {
    let mut advances = vec![0u16; NUM_GLYPHS as usize];
    advances[0] = ADVANCE_BASE * 2; // .notdef
    advances[GLYPH_TRAIL as usize] = ADVANCE_BASE * 2;
    advances[GLYPH_CAL_LOW as usize] = advance_for_byte(0);
    advances[GLYPH_CAL_HIGH as usize] = advance_for_byte(255);
    for i in 0..DATA_GLYPHS {
        advances[GLYPH_DATA_BASE as usize + i] = advance_for_byte(0);
    }
    advances
}

/// Build the baseline font written into every fresh bank slot.
pub fn build_baseline_font() -> Vec<u8> {
    build_font(&baseline_advances())
}

/// Build a font whose data glyphs encode the 512 bytes of a reply packet.
pub fn build_reply_font(bytes: &[u8]) -> Vec<u8> {
    assert_eq!(bytes.len(), DATA_GLYPHS, "reply packet must be 512 bytes");
    let mut advances = baseline_advances();
    for (i, &byte) in bytes.iter().enumerate() {
        advances[GLYPH_DATA_BASE as usize + i] = advance_for_byte(byte);
    }
    build_font(&advances)
}

/// Recover the data-byte encoded in glyph `i` from measured advances.
///
/// This mirrors the addon's calibration maths so the companion can round-trip
/// and self-test without a game client.
pub fn decode_byte(low: u16, high: u16, read: u16) -> u8 {
    if high <= low {
        return 0;
    }
    let span = (high - low) as f64;
    let value = ((read as f64 - low as f64) * 255.0 / span).round();
    value.clamp(0.0, 255.0) as u8
}

/// Parse a generated font and return the advance width of every glyph.
///
/// Used by tests and the `ocw dump` diagnostic command.
pub fn read_advances(font: &[u8]) -> Vec<u16> {
    let dir = parse_directory(font);
    let mut num_glyphs = 0u16;
    let mut num_hmetrics = 0u16;
    let mut hmtx_off = 0usize;

    for (tag, _, off, _) in &dir {
        match tag {
            b"maxp" => {
                let o = *off as usize;
                num_glyphs = u16::from_be_bytes([font[o + 4], font[o + 5]]);
            }
            b"hhea" => {
                let o = *off as usize;
                num_hmetrics = u16::from_be_bytes([font[o + 34], font[o + 35]]);
            }
            b"hmtx" => hmtx_off = *off as usize,
            _ => {}
        }
    }

    let mut advances = Vec::with_capacity(num_glyphs as usize);
    for i in 0..num_glyphs as usize {
        if (i as u16) < num_hmetrics {
            let o = hmtx_off + i * 4;
            advances.push(u16::from_be_bytes([font[o], font[o + 1]]));
        } else {
            // Monospaced tail: repeat the last advance.
            let o = hmtx_off + (num_hmetrics as usize - 1) * 4;
            advances.push(u16::from_be_bytes([font[o], font[o + 1]]));
        }
    }
    advances
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_font_is_structurally_valid() {
        let font = build_baseline_font();
        // 12-byte header + 10 tables * 16 bytes.
        assert_eq!(&font[0..4], &[0x00, 0x01, 0x00, 0x00]);
        let num_tables = u16::from_be_bytes([font[4], font[5]]);
        assert_eq!(num_tables, 10);

        let tags: Vec<[u8; 4]> = parse_directory(&font).iter().map(|d| d.0).collect();
        assert!(tags.contains(b"head"));
        assert!(tags.contains(b"glyf"));
        assert!(tags.contains(b"cmap"));
        assert!(tags.contains(b"hmtx"));

        let advances = read_advances(&font);
        assert_eq!(advances.len(), NUM_GLYPHS as usize);
    }

    #[test]
    fn glyphs_have_only_a_tiny_outline() {
        // Two constraints, both learned from live testing:
        //  * a large outline overflows the client's font atlas while measuring
        //    (a hard crash in GxuFontMiscClasses.cpp);
        //  * an outline-less glyph is treated as missing and substituted with a
        //    fallback, which destroys the advance measurement.
        let font = build_baseline_font();
        let dir = parse_directory(&font);
        let (_, _, offset, length) = dir
            .iter()
            .find(|(tag, _, _, _)| tag == b"glyf")
            .expect("glyf table");
        let data = &font[*offset as usize..(*offset + *length) as usize];
        assert_eq!(data.len(), NUM_GLYPHS as usize * GLYPH_LEN);
        for (index, chunk) in data.chunks(GLYPH_LEN).enumerate() {
            let contours = i16::from_be_bytes([chunk[0], chunk[1]]);
            assert_eq!(contours, 1, "glyph {index} must have exactly one contour");
            let x_max = i16::from_be_bytes([chunk[6], chunk[7]]);
            assert_eq!(x_max, GLYPH_BOX, "glyph {index} outline must stay tiny");
        }
    }

    #[test]
    fn head_checksum_adjustment_is_correct() {
        let font = build_baseline_font();
        assert_eq!(checksum(&font), 0xB1B0_AFBA);
    }

    #[test]
    fn table_checksums_match_directory() {
        let font = build_baseline_font();
        for (tag, sum, off, len) in parse_directory(&font) {
            let data = &font[off as usize..(off + len) as usize];
            if &tag == b"head" {
                // head's directory checksum is computed with checkSumAdjustment = 0.
                let mut copy = data.to_vec();
                copy[8..12].copy_from_slice(&[0, 0, 0, 0]);
                assert_eq!(checksum(&copy), sum, "head checksum");
            } else {
                assert_eq!(checksum(data), sum, "{} checksum", String::from_utf8_lossy(&tag));
            }
        }
    }

    #[test]
    fn reply_bytes_round_trip_through_font_metrics() {
        let mut packet = [0u8; DATA_GLYPHS];
        for (i, b) in packet.iter_mut().enumerate() {
            *b = ((i * 31 + 7) % 256) as u8;
        }
        let font = build_reply_font(&packet);
        let advances = read_advances(&font);

        let low = advances[GLYPH_CAL_LOW as usize];
        let high = advances[GLYPH_CAL_HIGH as usize];
        assert_eq!(low, advance_for_byte(0));
        assert_eq!(high, advance_for_byte(255));

        for (i, &byte) in packet.iter().enumerate() {
            let read = advances[GLYPH_DATA_BASE as usize + i];
            assert_eq!(decode_byte(low, high, read), byte, "byte {i}");
        }
    }

    #[test]
    fn all_byte_values_are_distinct() {
        let font = build_reply_font(&(0..=255).map(|b| b as u8).collect::<Vec<_>>().repeat(2)[..512].to_vec());
        let advances = read_advances(&font);
        let low = advances[GLYPH_CAL_LOW as usize];
        let high = advances[GLYPH_CAL_HIGH as usize];
        for b in 0..=255u8 {
            let read = advances[GLYPH_DATA_BASE as usize + b as usize];
            assert_eq!(decode_byte(low, high, read), b);
        }
    }
}
