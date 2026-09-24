//! Client for the OpenCode V2 HTTP API.
//!
//! Endpoints used (all scoped with the `x-opencode-directory` header):
//!
//! * `GET  /api/health`
//! * `GET  /api/project`
//! * `GET  /api/model`
//! * `POST /api/session`
//! * `POST /api/session/{id}/prompt`
//! * `GET  /api/session/{id}/message`
//! * `POST /api/session/{id}/interrupt`
//! * `DELETE /api/session/{id}`
//! * `GET  /api/vcs`, `GET /api/vcs/status`

use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::backend::{AskRequest, AskResponse};
use crate::http::Client;

/// A `provider/model` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    /// Parse a `provider/model` string.
    pub fn parse(s: &str) -> Option<Self> {
        let (provider_id, model_id) = s.split_once('/')?;
        if provider_id.is_empty() || model_id.is_empty() {
            return None;
        }
        Some(Self {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        })
    }

    /// JSON shape accepted by `POST /api/session` and `/model`.
    pub fn to_json(&self) -> Value {
        json!({ "id": self.model_id, "providerID": self.provider_id })
    }

    /// Human-readable label.
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// A live OpenCode server connection.
pub struct OpenCode {
    client: Client,
    project: Option<PathBuf>,
    model: Option<ModelRef>,
    session: Option<String>,
    timeout: Duration,
    poll_interval: Duration,
}

impl OpenCode {
    /// Connect to a server. `project` and `model` are defaults for new sessions.
    pub fn new(
        base_url: &str,
        password: Option<String>,
        project: Option<PathBuf>,
        model: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let mut client = Client::new(base_url, password)?;
        if let Some(dir) = &project {
            client.set_directory(Some(dir.display().to_string()));
        }
        Ok(Self {
            client,
            project,
            model: model.as_deref().and_then(ModelRef::parse),
            session: None,
            timeout: Duration::from_secs(timeout_secs.max(30)),
            poll_interval: Duration::from_millis(1500),
        })
    }

    /// A short human description for logging and the in-game status line.
    pub fn describe(&self) -> String {
        let model = self
            .model
            .as_ref()
            .map(|m| m.label())
            .unwrap_or_else(|| "default model".to_string());
        format!("OpenCode ({model})")
    }

    /// The model currently in use.
    pub fn model(&self) -> Option<&ModelRef> {
        self.model.as_ref()
    }

    /// The session this backend is bound to, if any.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Change the model used for subsequent sessions.
    pub fn set_model(&mut self, model: Option<ModelRef>) {
        self.model = model;
    }

    /// `GET /api/health`
    pub fn health(&self) -> Result<Value> {
        self.client.get_json("/api/health")
    }

    /// `GET /api/project`
    pub fn list_projects(&self) -> Result<Vec<Value>> {
        let value = self.client.get_json("/api/project")?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    /// `GET /api/model`
    pub fn list_models(&self) -> Result<Vec<ModelRef>> {
        let value = self.client.get_json("/api/model")?;
        let data = value
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        let mut models = Vec::new();
        for m in data {
            let Some(provider_id) = m.get("providerID").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(model_id) = m
                .get("modelID")
                .or_else(|| m.get("id"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let key = (provider_id.to_string(), model_id.to_string());
            if seen.insert(key.clone()) {
                models.push(ModelRef {
                    provider_id: key.0,
                    model_id: key.1,
                });
            }
        }
        Ok(models)
    }

    /// Fetch a single session's metadata.
    pub fn get_session(&self, session: &str) -> Result<Value> {
        let value = self.client.get_json(&format!("/api/session/{session}"))?;
        Ok(value.get("data").cloned().unwrap_or(value))
    }

    /// `GET /api/vcs`
    pub fn vcs_info(&self) -> Result<Value> {
        self.client.get_json("/api/vcs")
    }

    /// `GET /api/vcs/status`
    pub fn vcs_status(&self) -> Result<Value> {
        self.client.get_json("/api/vcs/status")
    }

    /// `DELETE /api/session/{id}`
    pub fn delete_session(&self, session: &str) -> Result<()> {
        self.client
            .delete(&format!("/api/session/{session}"))?
            .error_for_status()?;
        Ok(())
    }

    /// Create a new session in the configured project.
    ///
    /// The project must be sent in the body: the `x-opencode-directory` header
    /// scopes GET requests but is ignored by `POST /api/session`.
    pub fn create_session(&mut self, title: &str) -> Result<String> {
        let body = self.create_body(title);
        let response = self
            .client
            .post_json("/api/session", &body)?
            .error_for_status()?;
        let value = response.json()?;
        let id = value
            .get("data")
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .context("session create response missing data.id")?
            .to_string();
        self.session = Some(id.clone());
        Ok(id)
    }

    /// The JSON body used to create a session.
    fn create_body(&self, title: &str) -> Value {
        let mut body = json!({ "title": title });
        if let Some(model) = &self.model {
            body["model"] = model.to_json();
        }
        if let Some(project) = &self.project {
            body["location"] = json!({ "directory": project.display().to_string() });
        }
        body
    }

    /// Reuse the bound session or create one.
    pub fn ensure_session(&mut self) -> Result<String> {
        if let Some(id) = &self.session {
            return Ok(id.clone());
        }
        self.create_session("OCWow")
    }

    /// Send a prompt to a session.
    pub fn prompt(&self, session: &str, text: &str) -> Result<()> {
        self.client
            .post_json_timeout(
                &format!("/api/session/{session}/prompt"),
                &json!({ "text": text }),
                Duration::from_secs(60),
            )?
            .error_for_status()?;
        Ok(())
    }

    /// Interrupt the session's current turn.
    pub fn interrupt(&self, session: &str) -> Result<()> {
        self.client
            .post_json(
                &format!("/api/session/{session}/interrupt"),
                &json!({}),
            )?
            .error_for_status()?;
        Ok(())
    }

    /// Fetch a session's messages.
    pub fn messages(&self, session: &str) -> Result<Vec<Value>> {
        let value = self.client.get_json(&format!("/api/session/{session}/message"))?;
        Ok(value
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// The newest `time.created` across all messages, used to ignore stale replies.
    pub fn latest_created(&self, session: &str) -> Result<u64> {
        Ok(self
            .messages(session)?
            .iter()
            .filter_map(|m| created_of(m))
            .max()
            .unwrap_or(0))
    }

    /// Ask a question and wait for the assistant's reply.
    pub fn ask(&mut self, req: &AskRequest) -> Result<AskResponse> {
        // Re-scope the client when the requested project changes.
        if req.project != self.project {
            self.project = req.project.clone();
            self.client
                .set_directory(req.project.as_ref().map(|p| p.display().to_string()));
            self.session = None;
        }

        let session = match &req.session_id {
            Some(id) => {
                self.session = Some(id.clone());
                id.clone()
            }
            None => self.ensure_session()?,
        };

        let after = self.latest_created(&session)?;
        self.prompt(&session, &req.full_text())?;
        self.wait_for_reply(&session, after)
    }

    /// Poll a session until an assistant message newer than `after` completes.
    pub fn wait_for_reply(&self, session: &str, after: u64) -> Result<AskResponse> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let messages = self.messages(session)?;
            if let Some(reply) = extract_reply(&messages, after) {
                match reply.error {
                    Some(err) => bail!("agent reported an error: {err}"),
                    None if reply.done => {
                        return Ok(AskResponse {
                            text: reply.text,
                            session_id: Some(session.to_string()),
                        })
                    }
                    None => {}
                }
            }
            if Instant::now() >= deadline {
                bail!("timed out after {:?} waiting for a reply", self.timeout);
            }
            sleep(self.poll_interval);
        }
    }
}

/// `time.created` of a message, whether nested under `info` or top-level.
fn created_of(message: &Value) -> Option<u64> {
    let info = message.get("info").unwrap_or(message);
    info.get("time")
        .and_then(|t| t.get("created"))
        .and_then(|v| v.as_u64())
}

/// A reply extracted from a session's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedReply {
    pub created: u64,
    pub text: String,
    pub done: bool,
    pub error: Option<String>,
}

/// Find the newest assistant message created after `after`.
pub fn extract_reply(messages: &[Value], after: u64) -> Option<ExtractedReply> {
    let mut best: Option<ExtractedReply> = None;

    for message in messages {
        let info = message.get("info").unwrap_or(message);
        if info.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let created = created_of(message).unwrap_or(0);
        if created <= after {
            continue;
        }

        let parts = message
            .get("parts")
            .or_else(|| info.get("content"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let text: String = parts
            .iter()
            .filter(|p| p.get("type").and_then(|v| v.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("");

        let done = info
            .get("finish")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        let error = info
            .get("error")
            .filter(|v| !v.is_null())
            .map(|v| v.to_string());

        let candidate = ExtractedReply {
            created,
            text,
            done,
            error,
        };
        if best.as_ref().map(|b| created >= b.created).unwrap_or(true) {
            best = Some(candidate);
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_scopes_the_project() {
        let backend = OpenCode::new(
            "http://127.0.0.1:4096",
            None,
            Some(std::path::PathBuf::from("/tmp/proj")),
            Some("opencode/mimo-v2.6-flash-free".to_string()),
            60,
        )
        .unwrap();
        let body = backend.create_body("OCWow");
        assert_eq!(body["title"], "OCWow");
        // The directory must be in the body; the header alone does not scope
        // session creation.
        assert_eq!(body["location"]["directory"], "/tmp/proj");
        assert_eq!(body["model"]["providerID"], "opencode");
        assert_eq!(body["model"]["id"], "mimo-v2.6-flash-free");
    }

    #[test]
    fn parses_model_refs() {
        let m = ModelRef::parse("opencode/mimo-v2.6-flash-free").unwrap();
        assert_eq!(m.provider_id, "opencode");
        assert_eq!(m.model_id, "mimo-v2.6-flash-free");
        assert!(ModelRef::parse("nope").is_none());
        assert!(ModelRef::parse("/x").is_none());
    }

    #[test]
    fn extracts_completed_assistant_reply() {
        let messages = vec![
            json!({ "info": { "type": "user", "time": { "created": 100 } }, "parts": [] }),
            json!({
                "info": {
                    "type": "assistant",
                    "time": { "created": 200 },
                    "finish": "stop"
                },
                "parts": [
                    { "type": "text", "text": "pong" },
                    { "type": "tool", "tool": "bash" }
                ]
            }),
        ];
        let reply = extract_reply(&messages, 150).unwrap();
        assert_eq!(reply.text, "pong");
        assert!(reply.done);
        assert_eq!(reply.created, 200);
    }

    #[test]
    fn ignores_messages_before_the_prompt() {
        let messages = vec![json!({
            "info": { "type": "assistant", "time": { "created": 100 }, "finish": "stop" },
            "parts": [{ "type": "text", "text": "old" }]
        })];
        assert!(extract_reply(&messages, 150).is_none());
    }

    #[test]
    fn reports_streaming_and_errors() {
        let streaming = vec![json!({
            "info": { "type": "assistant", "time": { "created": 300 }, "finish": null },
            "parts": [{ "type": "text", "text": "par" }]
        })];
        let reply = extract_reply(&streaming, 150).unwrap();
        assert!(!reply.done);
        assert_eq!(reply.text, "par");

        let errored = vec![json!({
            "info": { "type": "assistant", "time": { "created": 400 }, "finish": "error", "error": { "message": "boom" } },
            "parts": []
        })];
        let reply = extract_reply(&errored, 150).unwrap();
        assert!(reply.error.is_some());
    }
}
