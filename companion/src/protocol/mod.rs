//! Wire protocol shared with the `OCWow` Lua addon.
//!
//! Outbound messages are the pixel strip ([`strip`]); inbound replies arrive
//! through the load-on-demand slot addons ([`crate::slots`]).

pub mod strip;
