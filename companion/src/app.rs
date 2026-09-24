//! The companion's main loop: decode the pixel strip, run prompts on a worker
//! thread, and serve replies back through the font bank.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::backend::opencode::ModelRef;
use crate::backend::{AskRequest, Backend};
use crate::capture::Capturer;
use crate::config::Config;
use crate::context;
use crate::fonts::Bank;
use crate::git;
use crate::protocol::adler32::adler32;
use crate::protocol::frames::{
    self, reply_state, ControlFrame, PromptFrame, ReplyFrame, OUT_TYPE_CONTROL, OUT_TYPE_PROMPT,
};
use crate::protocol::pixel::{self, GrayImage, BYTES_PER_FRAME, STRIP_COLS};
use crate::state::{find_wow_pid, State};

/// Bytes of reply text carried by one font packet.
const REPLY_CHUNK: usize = frames::REPLY_PAYLOAD_MAX;

/// How often the strip is sampled.
const DEFAULT_POLL_MS: u64 = 120;

/// Runtime state shared with the worker thread.
#[derive(Default)]
struct Shared {
    session_id: Mutex<Option<String>>,
    model: Mutex<Option<ModelRef>>,
    last_error: Mutex<Option<String>>,
}

/// Status of a single in-flight request.
#[derive(Debug, Clone)]
pub struct JobStatus {
    pub state: u8,
    pub payload: String,
    pub revision: u32,
    pub error: Option<String>,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self {
            state: reply_state::QUEUED,
            payload: String::new(),
            revision: 0,
            error: None,
        }
    }
}

/// A unit of work handed to the worker thread.
struct Job {
    ui_session: u16,
    request_id: u32,
    text: String,
    context: Option<String>,
    project: Option<PathBuf>,
    status: Arc<Mutex<JobStatus>>,
}

/// Messages the main loop can send to the worker.
enum WorkerMsg {
    Ask(Box<Job>),
    Interrupt,
    SetModel(String),
    NewSession,
}

/// Reassembly buffer for a multi-fragment prompt.
struct Partial {
    fragments: Vec<Option<Vec<u8>>>,
}

impl Partial {
    fn new(count: u16) -> Self {
        Self {
            fragments: vec![None; count.max(1) as usize],
        }
    }

    fn ensure_len(&mut self, count: u16) {
        let want = count.max(1) as usize;
        if self.fragments.len() != want {
            self.fragments.resize(want, None);
        }
    }

    fn is_complete(&self) -> bool {
        self.fragments.iter().all(|f| f.is_some())
    }

    fn join(&self) -> String {
        let mut bytes = Vec::new();
        for fragment in &self.fragments {
            if let Some(chunk) = fragment {
                bytes.extend_from_slice(chunk);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// The companion application.
pub struct App {
    config: Config,
    config_path: PathBuf,
    bank: Bank,
    capturer: Capturer,
    state: State,
    state_path: PathBuf,
    jobs: Arc<Mutex<HashMap<u64, Arc<Mutex<JobStatus>>>>>,
    shared: Arc<Shared>,
    worker_tx: Option<Sender<WorkerMsg>>,
    worker_rx: Option<Receiver<WorkerMsg>>,
    worker: Option<JoinHandle<()>>,
    backend: Option<Backend>,
    partial: HashMap<u64, Partial>,
    last_written: HashMap<u16, u32>,
    project: Option<PathBuf>,
    verbose: bool,
    poll_ms: u64,
}

impl App {
    /// Build the application, reconciling state against the running client.
    pub fn new(config: Config, backend: Backend, verbose: bool) -> Result<Self> {
        let bank = config.bank();
        let capturer = Capturer::new(
            config.capture.region(),
            config.capture.command.clone(),
            config.capture.cell_px,
        );
        let state_path = crate::state::default_state_path();
        let mut state = State::load(&state_path).unwrap_or_default();
        if state.observe_client(find_wow_pid()) {
            eprintln!("[ocw] detected a new WoW client process; will reset the addon slot counter");
        }

        let (worker_tx, worker_rx) = mpsc::channel();

        Ok(Self {
            project: config.opencode.project.clone(),
            config_path: crate::config::default_config_path(),
            config,
            bank,
            capturer,
            state,
            state_path,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            shared: Arc::new(Shared::default()),
            worker_tx: Some(worker_tx),
            worker_rx: Some(worker_rx),
            worker: None,
            backend: Some(backend),
            partial: HashMap::new(),
            last_written: HashMap::new(),
            verbose,
            poll_ms: DEFAULT_POLL_MS,
        })
    }

    /// Validate that the font bank is installed before running.
    pub fn check_bank(&self) -> Result<()> {        if !self.bank.is_installed() {
            anyhow::bail!(
                "font bank not installed at {}; run `ocw install` first",
                self.bank.dir().display()
            );
        }
        Ok(())
    }

    /// Override the strip sampling interval.
    pub fn set_poll_ms(&mut self, ms: u64) {
        self.poll_ms = ms.max(20);
    }

    /// Set the configuration file used to persist calibration results.
    pub fn set_config_path(&mut self, path: PathBuf) {
        self.config_path = path;
    }

    /// Start the worker thread without entering the capture loop.
    pub fn start(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let backend = self.backend.take().context("backend already moved")?;
        let worker_rx = self.worker_rx.take().context("worker already started")?;
        let shared = Arc::clone(&self.shared);
        let model = self.config.opencode.model.clone();
        self.worker = Some(thread::spawn(move || {
            worker_loop(backend, worker_rx, shared, model);
        }));
        Ok(())
    }

    /// Signal the worker to finish and join it.
    pub fn stop(&mut self) {
        self.worker_tx = None;
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    /// Feed a decoded frame into the bridge. Used by the self-test harness.
    #[doc(hidden)]
    pub fn ingest_frame(&mut self, bytes: &[u8; BYTES_PER_FRAME]) {
        self.handle_frame(bytes);
    }

    /// The font bank in use.
    #[doc(hidden)]
    pub fn bank(&self) -> &Bank {
        &self.bank
    }

    /// Current state of a request, for diagnostics and tests.
    #[doc(hidden)]
    pub fn job_state(&self, ui_session: u16, request_id: u32) -> Option<u8> {
        let key = job_key(ui_session, request_id);
        self.jobs
            .lock()
            .unwrap()
            .get(&key)
            .map(|status| status.lock().unwrap().state)
    }

    /// Backend description for the self-test banner.
    #[doc(hidden)]
    pub fn describe(&self) -> String {
        self.backend_description()
    }

    /// The session the backend is currently bound to.
    #[doc(hidden)]
    pub fn session_id(&self) -> Option<String> {
        self.shared.session_id.lock().unwrap().clone()
    }

    fn log(&self, message: &str) {
        if self.verbose {
            eprintln!("[ocw] {message}");
        }
    }

    /// Start the worker thread and run the main loop for `duration`.
    pub fn run(&mut self, duration: Option<Duration>) -> Result<()> {
        self.auto_calibrate();
        self.start()?;
        let result = self.main_loop(duration);
        self.stop();
        result
    }

    /// Best-effort one-shot calibration: if the configured crop does not decode
    /// cleanly, search the capture for the strip and adopt what is found.
    pub fn auto_calibrate(&mut self) {
        let image = match self.capturer.capture() {
            Ok(image) => image,
            Err(err) => {
                eprintln!("[ocw] calibration skipped: {err}");
                return;
            }
        };
        let gray = image.as_gray();

        if let Some(bytes) = decode_any(&gray, self.config.capture.cell_px) {
            if pixel::is_valid_frame(&bytes) {
                self.log("configured crop decodes cleanly");
                return;
            }
        }

        match pixel::find_strip(&gray) {
            Some((cell, dx, dy, _)) => {
                let region = self.config.capture.region();
                self.config.capture.x = region.x + dx as i32;
                self.config.capture.y = region.y + dy as i32;
                self.config.capture.width = STRIP_COLS as u32 * cell;
                self.config.capture.height = pixel::STRIP_ROWS as u32 * cell;
                self.config.capture.cell_px = 0;
                self.capturer = Capturer::new(
                    self.config.capture.region(),
                    self.config.capture.command.clone(),
                    0,
                );
                eprintln!(
                    "[ocw] auto-calibrated: cell={cell}px, crop {},{},{},{}",
                    self.config.capture.x,
                    self.config.capture.y,
                    self.config.capture.width,
                    self.config.capture.height
                );
                // Remember the crop so later runs start calibrated.
                if let Err(err) = self.config.save(&self.config_path) {
                    eprintln!("[ocw] could not save calibration: {err}");
                } else {
                    eprintln!("[ocw] saved to {}", self.config_path.display());
                }
            }
            None => eprintln!(
                "[ocw] warning: strip not found in the capture; run `ocw probe` \
                 (and `/ocw calibrate` in game)"
            ),
        }
    }

    fn main_loop(&mut self, duration: Option<Duration>) -> Result<()> {
        let started = Instant::now();
        let mut capture_errors = 0u32;
        let mut frames = 0u64;

        eprintln!(
            "[ocw] serving: bank={} slots at {}, backend={}, crop={:?}",
            self.bank.slots(),
            self.bank.dir().display(),
            self.backend_description(),
            self.config.capture.region()
        );
        eprintln!("[ocw] press Ctrl-C to stop");

        loop {
            if let Some(limit) = duration {
                if started.elapsed() >= limit {
                    break;
                }
            }

            match self.capturer.capture() {
                Ok(image) => {
                    capture_errors = 0;
                    if let Some(bytes) = decode_any(&image.as_gray(), self.config.capture.cell_px) {
                        frames += 1;
                        self.handle_frame(&bytes);
                    }
                }
                Err(err) => {
                    capture_errors += 1;
                    if capture_errors <= 3 || capture_errors % 50 == 0 {
                        eprintln!("[ocw] capture failed ({capture_errors}): {err}");
                        if capture_errors == 3 {
                            eprintln!(
                                "[ocw] hint: grant Screen Recording permission to your terminal, \
                                 and check the crop in `ocw probe`"
                            );
                        }
                    }
                    thread::sleep(Duration::from_millis(400));
                }
            }

            thread::sleep(Duration::from_millis(self.poll_ms));
        }

        eprintln!("[ocw] stopped after decoding {frames} frames");
        Ok(())
    }

    fn backend_description(&self) -> String {
        self.shared
            .model
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.label())
            .unwrap_or_else(|| "default".to_string())
    }

    fn send(&self, message: WorkerMsg) {
        if let Some(tx) = &self.worker_tx {
            let _ = tx.send(message);
        }
    }

    fn handle_frame(&mut self, bytes: &[u8; BYTES_PER_FRAME]) {
        match bytes[2] {
            OUT_TYPE_PROMPT => match PromptFrame::decode(bytes) {
                Ok(frame) => self.on_prompt(frame),
                Err(err) => self.log(&format!("bad prompt frame: {err}")),
            },
            OUT_TYPE_CONTROL => match ControlFrame::decode(bytes) {
                Ok(frame) => self.on_control(frame),
                Err(err) => self.log(&format!("bad control frame: {err}")),
            },
            other => self.log(&format!("unknown frame type {other}")),
        }
    }

    fn on_prompt(&mut self, frame: PromptFrame) {
        let key = job_key(frame.ui_session, frame.request_id);
        let partial = self.partial.entry(key).or_insert_with(|| Partial::new(frame.fragment_count));
        partial.ensure_len(frame.fragment_count);

        if (frame.fragment_index as usize) < partial.fragments.len() {
            partial.fragments[frame.fragment_index as usize] = Some(frame.payload.clone());
        }

        if !partial.is_complete() {
            return;
        }

        let text = partial.join();
        self.partial.remove(&key);

        if self.jobs.lock().unwrap().contains_key(&key) {
            return;
        }

        self.log(&format!(
            "request {} (session {}): {} bytes",
            frame.request_id,
            frame.ui_session,
            text.len()
        ));
        self.start_job(key, frame.ui_session, frame.request_id, text);
    }

    fn start_job(&mut self, key: u64, ui_session: u16, request_id: u32, text: String) {
        let status = Arc::new(Mutex::new(JobStatus::default()));
        self.jobs
            .lock()
            .unwrap()
            .insert(key, Arc::clone(&status));

        // Local slash-commands are answered without touching the backend.
        if let Some(reply) = self.local_command(&text) {
            let mut guard = status.lock().unwrap();
            guard.state = reply_state::DONE;
            guard.payload = reply;
            guard.revision = revision_of(guard.state, &guard.payload);
            return;
        }

        let (user_text, game_state) = context::split_game_state(&text);
        let git_summary = self.project.as_deref().and_then(git::summarize);
        let ctx = context::compose(
            &self.backend_description(),
            self.project.as_deref(),
            git_summary.as_ref(),
            game_state.as_deref(),
        );

        let job = Job {
            ui_session,
            request_id,
            text: user_text,
            context: Some(ctx),
            project: self.project.clone(),
            status,
        };
        if let Some(tx) = &self.worker_tx {
            let _ = tx.send(WorkerMsg::Ask(Box::new(job)));
        }
    }

    fn local_command(&mut self, text: &str) -> Option<String> {
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        let (command, arg) = match trimmed.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (trimmed, ""),
        };

        match command {
            "/help" => Some(
                "commands: /help /stop /new /model <provider/model> /status".to_string(),
            ),
            "/stop" | "/interrupt" => {
                self.send(WorkerMsg::Interrupt);
                Some("interrupt requested".to_string())
            }
            "/new" => {
                self.send(WorkerMsg::NewSession);
                Some("new session started".to_string())
            }
            "/model" => {
                if arg.is_empty() {
                    return Some(format!(
                        "current model: {}",
                        self.backend_description()
                    ));
                }
                self.send(WorkerMsg::SetModel(arg.to_string()));
                Some(format!("model set to {arg}"))
            }
            "/status" => {
                let session = self
                    .shared
                    .session_id
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "none".to_string());
                let project = self
                    .project
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "none".to_string());
                Some(format!(
                    "backend: {}\nsession: {session}\nproject: {project}\nmodel: {}",
                    "opencode",
                    self.backend_description()
                ))
            }
            other => Some(format!("unknown command {other}; try /help")),
        }
    }

    fn on_control(&mut self, control: ControlFrame) {
        // A detected client restart rewinds the addon's slot counter.
        if self.state.pending_reset {
            let frame = ReplyFrame {
                state: reply_state::RESET,
                ui_session: control.ui_session,
                request_id: control.request_id,
                fragment_index: 1,
                fragment_count: 1,
                revision: 1,
                slot: control.slot,
                flags: 0,
                payload: b"reset".to_vec(),
            };
            let _ = self.bank.publish_reply(control.slot, &frame.encode());
            if control.slot <= 2 {
                self.state.pending_reset = false;
                let _ = self.state.save(&self.state_path);
            }
            return;
        }

        let key = job_key(control.ui_session, control.request_id);
        let (state, payload, revision) = {
            let jobs = self.jobs.lock().unwrap();
            match jobs.get(&key) {
                Some(status) => {
                    let guard = status.lock().unwrap();
                    (guard.state, guard.payload.clone(), guard.revision)
                }
                None => (reply_state::WAITING, String::new(), 0),
            }
        };

        let count = fragment_count(payload.len());
        let mut index = control.requested_fragment;
        if index < 1 || index > count {
            index = 1;
        }
        let chunk = fragment_at(&payload, index);

        let frame = ReplyFrame {
            state,
            ui_session: control.ui_session,
            request_id: control.request_id,
            fragment_index: index,
            fragment_count: count,
            revision,
            slot: control.slot,
            flags: 0,
            payload: chunk,
        };
        let encoded = frame.encode();

        let fingerprint = adler32(&encoded[..60]);
        if self.last_written.get(&control.slot) == Some(&fingerprint) {
            return;
        }

        match self.bank.publish_reply(control.slot, &encoded) {
            Ok(()) => {
                self.last_written.insert(control.slot, fingerprint);
                self.log(&format!(
                    "wrote {} fragment {index}/{count} to slot {}",
                    reply_state::name(state),
                    control.slot
                ));
            }
            Err(err) => eprintln!("[ocw] failed to write slot {}: {err}", control.slot),
        }
    }
}

/// Owns the backend inside the worker thread.
fn worker_loop(
    mut backend: Backend,
    rx: Receiver<WorkerMsg>,
    shared: Arc<Shared>,
    initial_model: Option<String>,
) {
    if let Some(model) = initial_model.as_deref().and_then(ModelRef::parse) {
        *shared.model.lock().unwrap() = Some(model);
    }

    while let Ok(message) = rx.recv() {
        match message {
            WorkerMsg::Ask(job) => {
                let status = Arc::clone(&job.status);
                {
                    let mut guard = status.lock().unwrap();
                    guard.state = reply_state::WORKING;
                    guard.revision = revision_of(guard.state, &guard.payload);
                }

                let session = shared.session_id.lock().unwrap().clone();
                let request = AskRequest {
                    ui_session: job.ui_session,
                    request_id: job.request_id,
                    text: job.text,
                    context: job.context,
                    project: job.project,
                    model: shared.model.lock().unwrap().as_ref().map(|m| m.label()),
                    session_id: session,
                };

                match backend.ask(&request) {
                    Ok(response) => {
                        if let Some(session) = &response.session_id {
                            *shared.session_id.lock().unwrap() = Some(session.clone());
                        }
                        let mut guard = status.lock().unwrap();
                        guard.state = reply_state::DONE;
                        guard.payload = response.text;
                        guard.revision = revision_of(guard.state, &guard.payload);
                        *shared.last_error.lock().unwrap() = None;
                    }
                    Err(err) => {
                        let message = err.to_string();
                        let mut guard = status.lock().unwrap();
                        guard.state = reply_state::FAILED;
                        guard.payload = format!("error: {message}");
                        guard.error = Some(message.clone());
                        guard.revision = revision_of(guard.state, &guard.payload);
                        *shared.last_error.lock().unwrap() = Some(message);
                    }
                }
            }
            WorkerMsg::Interrupt => {
                if let Some(session) = shared.session_id.lock().unwrap().clone() {
                    let _ = backend.interrupt(&session);
                }
            }
            WorkerMsg::SetModel(spec) => {
                let parsed = ModelRef::parse(&spec);
                *shared.model.lock().unwrap() = parsed;
            }
            WorkerMsg::NewSession => {
                *shared.session_id.lock().unwrap() = None;
            }
        }
    }
}

/// Stable key for a `(ui_session, request_id)` pair.
pub fn job_key(ui_session: u16, request_id: u32) -> u64 {
    ((ui_session as u64) << 32) | request_id as u64
}

/// Number of fragments needed for a payload.
pub fn fragment_count(len: usize) -> u16 {
    ((len + REPLY_CHUNK - 1) / REPLY_CHUNK).max(1) as u16
}

/// Extract the 1-based fragment `index` from a payload.
pub fn fragment_at(payload: &str, index: u16) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let start = (index.saturating_sub(1) as usize) * REPLY_CHUNK;
    if start >= bytes.len() {
        return Vec::new();
    }
    bytes[start..(start + REPLY_CHUNK).min(bytes.len())].to_vec()
}

/// A content-derived revision so the addon can discard partial assemblies.
fn revision_of(state: u8, payload: &str) -> u32 {
    (adler32(payload.as_bytes()) ^ (state as u32)).max(1)
}

/// Decode one frame, auto-detecting the cell size when `hint` is zero.
fn decode_any(image: &GrayImage, hint: u32) -> Option<[u8; BYTES_PER_FRAME]> {
    if hint > 0 {
        return pixel::decode_strip(image, hint);
    }
    let auto = image.width / STRIP_COLS as u32;
    if auto == 0 {
        None
    } else {
        pixel::decode_strip(image, auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_split_and_count() {
        let payload = "a".repeat(REPLY_CHUNK + 10);
        assert_eq!(fragment_count(payload.len()), 2);
        assert_eq!(fragment_at(&payload, 1).len(), REPLY_CHUNK);
        assert_eq!(fragment_at(&payload, 2).len(), 10);
        assert!(fragment_at(&payload, 3).is_empty());
    }

    #[test]
    fn empty_payload_is_one_fragment() {
        assert_eq!(fragment_count(0), 1);
    }

    #[test]
    fn partial_reassembly() {
        let mut partial = Partial::new(2);
        partial.fragments[0] = Some(b"hello ".to_vec());
        assert!(!partial.is_complete());
        partial.fragments[1] = Some(b"world".to_vec());
        assert!(partial.is_complete());
        assert_eq!(partial.join(), "hello world");
    }

    #[test]
    fn revision_changes_with_content() {
        assert_ne!(revision_of(reply_state::DONE, "a"), revision_of(reply_state::DONE, "b"));
        assert_ne!(revision_of(reply_state::DONE, "a"), revision_of(reply_state::WORKING, "a"));
    }

    #[test]
    fn job_key_is_unique() {
        assert_ne!(job_key(1, 1), job_key(1, 2));
        assert_ne!(job_key(1, 1), job_key(2, 1));
    }
}
