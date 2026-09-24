//! Answer backends: the real OpenCode server, or a local mock for transport
//! testing without an agent.

pub mod mock;
pub mod opencode;

use std::path::PathBuf;

use anyhow::Result;

pub use mock::Mock;
pub use opencode::{ModelRef, OpenCode};

/// A question routed from the addon to a backend.
#[derive(Debug, Clone)]
pub struct AskRequest {
    pub ui_session: u16,
    pub request_id: u32,
    /// The user's prompt text.
    pub text: String,
    /// Optional situational context block prepended to the prompt.
    pub context: Option<String>,
    /// Project directory to run in.
    pub project: Option<PathBuf>,
    /// Model reference (`provider/model`).
    pub model: Option<String>,
    /// Continue an existing session instead of creating a new one.
    pub session_id: Option<String>,
}

impl AskRequest {
    /// The full prompt text actually sent to the model.
    pub fn full_text(&self) -> String {
        match &self.context {
            Some(ctx) if !ctx.trim().is_empty() => format!("{ctx}\n\n{}", self.text),
            _ => self.text.clone(),
        }
    }
}

/// A backend's answer.
#[derive(Debug, Clone)]
pub struct AskResponse {
    pub text: String,
    pub session_id: Option<String>,
}

/// Either the mock backend or a live OpenCode server.
pub enum Backend {
    Mock(Mock),
    OpenCode(Box<OpenCode>),
}

impl Backend {
    pub fn ask(&mut self, req: &AskRequest) -> Result<AskResponse> {
        match self {
            Backend::Mock(b) => b.ask(req),
            Backend::OpenCode(b) => b.ask(req),
        }
    }

    pub fn interrupt(&mut self, session_id: &str) -> Result<()> {
        match self {
            Backend::Mock(_) => Ok(()),
            Backend::OpenCode(b) => b.interrupt(session_id),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Backend::Mock(_) => "mock (local echo)".to_string(),
            Backend::OpenCode(b) => b.describe(),
        }
    }
}
