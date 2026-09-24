//! Screen capture and PNG decoding for the outbound channel.

pub mod png;
pub mod screen;

pub use png::{decode_png, DecodedImage};
pub use screen::{Capturer, Region};
