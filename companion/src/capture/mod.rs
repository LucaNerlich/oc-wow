//! Screen capture, PNG decoding, and strip location.

pub mod png;
pub mod scan;
pub mod screen;

pub use png::{decode_png, RgbImage};
pub use scan::{find_strip, sample_strip, StripLocation};
pub use screen::{Capturer, Region};
