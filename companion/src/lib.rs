//! OCWow companion library.
//!
//! The companion bridges a sandboxed World of Warcraft addon and a local
//! OpenCode server. It speaks two transports:
//!
//! * **Outbound (addon -> companion):** the addon paints a 128x4 black/white
//!   cell strip on screen; the companion screen-captures that region and
//!   decodes [`protocol::frames::PromptFrame`] / [`protocol::frames::ControlFrame`].
//! * **Inbound (companion -> addon):** the companion writes a TrueType font
//!   whose glyph advance widths encode bytes; the addon loads the font and
//!   measures it with `GetStringWidth` to recover [`protocol::frames::ReplyFrame`].
//!
//! Nothing here injects input, touches game memory, or reads process memory.

pub mod app;
pub mod backend;
pub mod capture;
pub mod config;
pub mod context;
pub mod fonts;
pub mod git;
pub mod http;
pub mod protocol;
pub mod state;

pub use config::Config;
