//! End-to-end transport round-trips.
//!
//! These exercise the two channels the way the addon and companion use them,
//! without a game client:
//!
//! * outbound: encode a prompt frame, paint it into a synthetic screenshot,
//!   decode it back through the strip sampler;
//! * inbound: encode a reply frame into a font, read the glyph advances, and
//!   recover the bytes with the same calibration maths the Lua receiver uses.

use ocw::fonts::{build_reply_font, read_advances, ADVANCE_BASE, GLYPH_CAL_HIGH, GLYPH_CAL_LOW, GLYPH_DATA_BASE, DATA_GLYPHS};
use ocw::protocol::frames::{reply_state, ControlFrame, PromptFrame, ReplyFrame};
use ocw::protocol::pixel::{
    bytes_to_cells, decode_strip, GrayImage, BYTES_PER_FRAME, STRIP_COLS, STRIP_ROWS,
};

/// Paint a frame into a synthetic screenshot with a given cell size and padding.
fn paint(bytes: &[u8; BYTES_PER_FRAME], cell: u32, pad: u32) -> (Vec<u8>, u32, u32) {
    let width = STRIP_COLS as u32 * cell + pad * 2;
    let height = STRIP_ROWS as u32 * cell + pad * 2;
    let mut image = vec![0u8; (width * height) as usize];
    let cells = bytes_to_cells(bytes);
    for row in 0..STRIP_ROWS as u32 {
        for col in 0..STRIP_COLS as u32 {
            if cells[row as usize * STRIP_COLS + col as usize] {
                for dy in 0..cell {
                    for dx in 0..cell {
                        let x = pad + col * cell + dx;
                        let y = pad + row * cell + dy;
                        image[(y * width + x) as usize] = 255;
                    }
                }
            }
        }
    }
    (image, width, height)
}

#[test]
fn prompt_survives_the_pixel_channel() {
    let prompt = "what quest am I on? context: zone Elwynn Forest, level 12";
    let fragment = &prompt.as_bytes()[..40.min(prompt.len())];
    let frame = PromptFrame {
        ui_session: 4242,
        request_id: 7,
        fragment_index: 0,
        fragment_count: 1,
        flags: 0,
        payload: fragment.to_vec(),
    };
    let bytes = frame.encode();

    let (image, width, height) = paint(&bytes, 4, 12);
    let gray = GrayImage::new(width, height, &image);
    let view = gray.sub(12, 12, STRIP_COLS as u32 * 4, STRIP_ROWS as u32 * 4);
    let decoded = decode_strip(&view, 4).expect("frame decodes");
    assert_eq!(decoded, bytes);

    let round_tripped = PromptFrame::decode(&decoded).expect("frame parses");
    assert_eq!(round_tripped, frame);
}

#[test]
fn control_frame_survives_the_pixel_channel() {
    let frame = ControlFrame {
        ui_session: 1,
        request_id: 99,
        requested_fragment: 2,
        slot: 512,
        deadline_ms: 1500,
        state: reply_state::WORKING,
        active: true,
        last_attempted_slot: 511,
    };
    let bytes = frame.encode();
    let (image, width, height) = paint(&bytes, 6, 0);
    let gray = GrayImage::new(width, height, &image);
    let decoded = decode_strip(&gray, 6).expect("frame decodes");
    assert_eq!(ControlFrame::decode(&decoded).unwrap(), frame);
}

#[test]
fn reply_survives_the_font_channel() {
    let reply = ReplyFrame {
        state: reply_state::DONE,
        ui_session: 4242,
        request_id: 7,
        fragment_index: 1,
        fragment_count: 1,
        revision: 0xABCD_1234,
        slot: 33,
        flags: 0,
        payload: b"Here is the answer: kill 10 wolves, then return to Goldshire.".to_vec(),
    };
    let packet = reply.encode();
    let font = build_reply_font(&packet);

    // Mirror the Lua receiver: measure calibration glyphs and data glyphs,
    // then normalise. The trailing glyph cancels, so it is omitted here.
    let advances = read_advances(&font);
    let low = advances[GLYPH_CAL_LOW as usize];
    let high = advances[GLYPH_CAL_HIGH as usize];
    assert_eq!(low, ADVANCE_BASE);
    assert_eq!(high, ADVANCE_BASE + 16 * 255);

    let mut recovered = [0u8; DATA_GLYPHS];
    for (i, slot) in recovered.iter_mut().enumerate() {
        let read = advances[GLYPH_DATA_BASE as usize + i];
        *slot = ocw::fonts::decode_byte(low, high, read);
    }

    assert_eq!(&recovered[..], &packet[..]);
    assert_eq!(ReplyFrame::decode(&recovered).unwrap(), reply);
}

#[test]
fn long_reply_fragments_reassemble() {
    // Simulate a reply longer than one font packet.
    let text: String = "The quick brown fox jumps over the lazy dog. ".repeat(30);
    let bytes = text.as_bytes();
    let chunk = ocw::protocol::frames::REPLY_PAYLOAD_MAX;
    let total = (bytes.len() + chunk - 1) / chunk;

    let mut assembled = String::new();
    for index in 0..total {
        let start = index * chunk;
        let end = (start + chunk).min(bytes.len());
        let frame = ReplyFrame {
            state: reply_state::DONE,
            ui_session: 1,
            request_id: 1,
            fragment_index: (index + 1) as u16,
            fragment_count: total as u16,
            revision: 1,
            slot: index as u16 + 1,
            flags: 0,
            payload: bytes[start..end].to_vec(),
        };
        let packet = frame.encode();
        let font = build_reply_font(&packet);
        let advances = read_advances(&font);
        let low = advances[GLYPH_CAL_LOW as usize];
        let high = advances[GLYPH_CAL_HIGH as usize];
        let mut recovered = [0u8; DATA_GLYPHS];
        for (i, slot) in recovered.iter_mut().enumerate() {
            *slot = ocw::fonts::decode_byte(low, high, advances[GLYPH_DATA_BASE as usize + i]);
        }
        let decoded = ReplyFrame::decode(&recovered).unwrap();
        assembled.push_str(&String::from_utf8_lossy(&decoded.payload));
    }

    assert_eq!(assembled, text);
}
