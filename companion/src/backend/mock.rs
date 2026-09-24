//! A local echo backend used to validate the transport without an agent.

use anyhow::Result;

use crate::backend::{AskRequest, AskResponse};

/// Echoes the prompt back, optionally with the received context.
#[derive(Debug, Default)]
pub struct Mock {
    /// When true, the reply includes the context block that was attached.
    pub echo_context: bool,
}

impl Mock {
    pub fn new() -> Self {
        Self {
            echo_context: false,
        }
    }

    pub fn ask(&mut self, req: &AskRequest) -> Result<AskResponse> {
        let mut text = format!("mock reply #{}: {}", req.request_id, req.text);
        if self.echo_context {
            if let Some(ctx) = &req.context {
                text.push_str("\n\n--- context ---\n");
                text.push_str(ctx);
            }
        }
        Ok(AskResponse {
            text,
            session_id: req.session_id.clone(),
        })
    }
}
