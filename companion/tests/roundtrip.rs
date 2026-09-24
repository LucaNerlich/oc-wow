//! End-to-end transport round-trips.
//!
//! Outbound: the addon paints a message as coloured cells; the companion finds
//! the strip in a screenshot, decodes the frame and parses the records.
//! Inbound: the companion writes slot data, which the addon reads back.

use ocw::app::Record;
use ocw::capture::png::RgbImage;
use ocw::capture::{find_strip, sample_strip};
use ocw::protocol::strip::{
    bytes_to_cells, cell_color, decode_frame, encode_frame, CELLS_PER_ROW, MAGIC,
};
use ocw::slots::{render_slot, Reply, ReplyStatus, SlotData};

/// Paint a frame into a synthetic screenshot at a given cell size and offset.
fn paint(frame: &[u8], cell_px: u32, offset: (u32, u32), pad: u32) -> RgbImage {
    let cells = bytes_to_cells(frame);
    let rows = (cells.len() + CELLS_PER_ROW - 1) / CELLS_PER_ROW;
    let width = CELLS_PER_ROW as u32 * cell_px + pad * 2 + offset.0;
    let height = rows as u32 * cell_px + pad * 2 + offset.1;
    let mut data = vec![0u8; (width * height * 3) as usize];

    for (index, &value) in cells.iter().enumerate() {
        let (r, g, b) = cell_color(value);
        let (cr, cg, cb) = ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
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

/// Mirror of the addon's `Codec.Record`.
fn addon_record(fields: &[&str]) -> String {
    fields.join("\u{1f}")
}

#[test]
fn a_prompt_survives_the_pixel_strip() {
    let record = addon_record(&["sess1", "2", "7", "", "", "Chat 2", "what quest am I on?"]);
    let frame = encode_frame(4242, record.as_bytes());

    let image = paint(&frame, 4, (61, 23), 12);
    let location = find_strip(&image).expect("strip found");
    assert_eq!(location.cell_px, 4);

    let cells = sample_strip(&image, &location);
    let (id, payload) = decode_frame(&bytes_to_cells_round_trip(&cells)).unwrap();
    assert_eq!(id, 4242);

    let records = Record::parse_all(&payload);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tab, 2);
    assert_eq!(records[0].request, 7);
    assert_eq!(records[0].session, "sess1");
    assert_eq!(records[0].text, "what quest am I on?");
}

fn bytes_to_cells_round_trip(cells: &[u8]) -> Vec<u8> {
    ocw::protocol::strip::cells_to_bytes(cells)
}

#[test]
fn several_records_fit_in_one_frame() {
    let first = addon_record(&["s", "1", "1", "", "", "Chat 1", "first"]);
    let second = addon_record(&["s", "1", "2", "", "c", "Chat 1", "zone: Elwynn", "second"]);
    let payload = format!("{first}\u{1e}{second}");
    let frame = encode_frame(9, payload.as_bytes());

    let image = paint(&frame, 4, (0, 0), 0);
    let location = find_strip(&image).unwrap();
    let cells = sample_strip(&image, &location);
    let (_, decoded) = decode_frame(&bytes_to_cells_round_trip(&cells)).unwrap();

    let records = Record::parse_all(&decoded);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].context.as_deref(), Some("zone: Elwynn"));
    assert_eq!(records[1].text, "second");
}

#[test]
fn utf8_survives_the_strip() {
    let text = "Warcraft: Ærøskøbing 🐉";
    let record = addon_record(&["s", "1", "1", "", "", "Chat", text]);
    let frame = encode_frame(1, record.as_bytes());
    let image = paint(&frame, 4, (0, 0), 0);
    let location = find_strip(&image).unwrap();
    let cells = sample_strip(&image, &location);
    let (_, payload) = decode_frame(&bytes_to_cells_round_trip(&cells)).unwrap();
    let records = Record::parse_all(&payload);
    assert_eq!(records[0].text, text);
}

#[test]
fn the_magic_is_what_calibration_looks_for() {
    // Every frame starts with the same colours, which is how the strip is found.
    let cells = bytes_to_cells(&MAGIC);
    assert_eq!(cells, vec![0b110, 0b001, 0b110, 0b001, 0b101, 0b000]);
}

#[test]
fn slot_data_renders_as_loadable_lua() {
    let data = SlotData {
        now: 1_700_000_000,
        seq: 12,
        replies: vec![
            Reply {
                tab: 1,
                request: 3,
                status: ReplyStatus::Done,
                text: "first line\nsecond \"quoted\" line".to_string(),
                session: "ses_abc".to_string(),
                denied: vec!["Bash(rm:*)".to_string()],
            },
            Reply {
                tab: 2,
                request: 4,
                status: ReplyStatus::Working,
                text: String::new(),
                session: String::new(),
                denied: Vec::new(),
            },
        ],
    };
    let lua = render_slot(&data);
    assert!(lua.starts_with("-- generated"));
    assert!(lua.contains("OCWow_SlotData = {"));
    assert!(lua.contains("now = 1700000000"));
    assert!(lua.contains("tab = 1"));
    assert!(lua.contains("status = \"done\""));
    // Quotes and newlines must be escaped so the file stays valid Lua.
    assert!(lua.contains(r#"first line\nsecond \"quoted\" line"#));
    assert!(lua.contains(r#"denied = {"Bash(rm:*)"}"#));
    assert!(lua.ends_with("}\n"));
}

#[test]
fn long_replies_are_carried_whole() {
    // Slot files have no size limit, unlike the old font channel.
    let text: String = "The quick brown fox jumps over the lazy dog. ".repeat(200);
    let data = SlotData {
        now: 1,
        seq: 1,
        replies: vec![Reply {
            tab: 1,
            request: 1,
            status: ReplyStatus::Done,
            text: text.clone(),
            session: "s".to_string(),
            denied: Vec::new(),
        }],
    };
    let lua = render_slot(&data);
    assert!(lua.contains(&text));
}
