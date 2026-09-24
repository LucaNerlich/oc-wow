//! OCWow companion library.
//!
//! The companion bridges a sandboxed World of Warcraft addon and a local
//! OpenCode server:
//!
//! * **Outbound** the addon paints messages as a strip of coloured cells in the
//!   top-left of the game window; the companion screen-captures that region and
//!   decodes it ([`capture`], [`protocol::strip`]).
//! * **Inbound** the companion writes the latest state of every chat into a bank
//!   of load-on-demand slot addons, which the game reads from disk when it loads
//!   one ([`slots`]).
//!
//! Nothing here injects input, touches game memory, or reads process memory.

pub mod app;
pub mod backend;
pub mod capture;
pub mod config;
pub mod context;
pub mod git;
pub mod http;
pub mod protocol;
pub mod slots;
pub mod state;

pub use config::Config;
