//! The companion's main loop: decode the pixel strip, run prompts on a worker
//! thread, and publish replies through the load-on-demand slot bank.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::backend::opencode::ModelRef;
use crate::backend::{AskRequest, Backend};
use crate::capture::{find_strip, sample_strip, Capturer, Region, StripLocation};
use crate::config::{Config, StripSettings};
use crate::context;
use crate::git;
use crate::protocol::strip::{self, StripError};
use crate::slots::{Reply, ReplyStatus, SlotBank, SlotData};

/// How often the strip is sampled.
const DEFAULT_POLL_MS: u64 = 250;
/// How often slot data is published even when nothing changed.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(1);
/// Margin, in points, added around the strip when caching its screen rectangle.
const REGION_MARGIN: i32 = 24;

/// Runtime state shared with the worker thread.
#[derive(Default)]
struct Shared {
    /// One OpenCode session per tab.
    sessions: Mutex<HashMap<u8, String>>,
    model: Mutex<Option<ModelRef>>,
}

/// Status of a single in-flight request.
#[derive(Debug, Clone)]
pub struct JobStatus {
    pub status: ReplyStatus,
    pub payload: String,
    pub session: String,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self {
            status: ReplyStatus::Working,
            payload: String::new(),
            session: String::new(),
        }
    }
}

/// A unit of work handed to the worker thread.
struct Job {
    tab: u8,
    request: u32,
    text: String,
    context: Option<String>,
    project: Option<PathBuf>,
    status: Arc<Mutex<JobStatus>>,
}

/// Messages the main loop can send to the worker.
enum WorkerMsg {
    Ask(Box<Job>),
    Interrupt(u8),
    SetModel { tab: u8, spec: String },
    NewSession(u8),
    CloseSession(u8),
}

/// One record decoded from the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub session: String,
    pub tab: u8,
    pub request: u32,
    pub cwd: String,
    pub flags: String,
    pub name: String,
    pub context: Option<String>,
    pub text: String,
}

impl Record {
    pub fn is_hello(&self) -> bool {
        self.flags.contains('h')
    }

    pub fn is_delete(&self) -> bool {
        self.flags.contains('d')
    }

    pub fn starts_new_session(&self) -> bool {
        self.flags.contains('n')
    }

    /// Split a decoded payload into records.
    pub fn parse_all(payload: &[u8]) -> Vec<Record> {
        strip::parse_records(payload)
            .into_iter()
            .filter_map(|fields| Record::from_fields(&fields))
            .collect()
    }

    fn from_fields(fields: &[Vec<u8>]) -> Option<Record> {
        // session, tab, request, cwd, flags, name, [context,] text
        if fields.len() < 7 {
            return None;
        }
        let text_of = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        let flags = text_of(&fields[4]);
        let has_context = flags.split(';').any(|f| f == "c");
        let context = if has_context && fields.len() >= 8 {
            Some(text_of(&fields[6]))
        } else {
            None
        };
        let text_start = if has_context { 7 } else { 6 };
        let text = fields[text_start..]
            .iter()
            .map(|f| text_of(f))
            .collect::<Vec<_>>()
            .join("\u{1f}");

        Some(Record {
            session: text_of(&fields[0]),
            tab: text_of(&fields[1]).parse().ok()?,
            request: text_of(&fields[2]).parse().ok()?,
            cwd: text_of(&fields[3]),
            flags,
            name: text_of(&fields[5]),
            context,
            text,
        })
    }
}

/// The companion application.
pub struct App {
    config: Config,
    bank: SlotBank,
    capturer: Capturer,
    strip: Option<StripSettings>,
    jobs: Arc<Mutex<HashMap<JobKey, Arc<Mutex<JobStatus>>>>>,
    shared: Arc<Shared>,
    worker_tx: Option<Sender<WorkerMsg>>,
    worker_rx: Option<Receiver<WorkerMsg>>,
    worker: Option<JoinHandle<()>>,
    backend: Option<Backend>,
    project: Option<PathBuf>,
    seen: HashSet<u32>,
    signaled: HashSet<(u8, u32)>,
    slot_seq: u64,
    last_published: String,
    last_publish: Instant,
    verbose: bool,
    poll_ms: u64,
}

/// Stable key for a `(tab, request)` pair.
pub type JobKey = (u8, u32);

pub fn job_key(tab: u8, request: u32) -> JobKey {
    (tab, request)
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl App {
    /// Build the application, reconciling state against the running client.
    pub fn new(config: Config, backend: Backend, verbose: bool) -> Result<Self> {
        let bank = config.slot_bank();
        let capturer = Capturer::new(config.capture.command.clone());
        let strip = config.capture.strip;

        let (worker_tx, worker_rx) = mpsc::channel();

        Ok(Self {
            project: config.opencode.project.clone(),
            config,
            bank,
            capturer,
            strip,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            shared: Arc::new(Shared::default()),
            worker_tx: Some(worker_tx),
            worker_rx: Some(worker_rx),
            worker: None,
            backend: Some(backend),
            seen: HashSet::new(),
            signaled: HashSet::new(),
            slot_seq: 0,
            last_published: String::new(),
            last_publish: Instant::now(),
            verbose,
            poll_ms: DEFAULT_POLL_MS,
        })
    }

    /// Validate that the slots are installed before running.
    pub fn check_slots(&self) -> Result<()> {
        if !self.bank.is_installed() {
            anyhow::bail!(
                "slot addons not installed under {}; run `ocw install` first",
                self.bank.addons_dir().display()
            );
        }
        Ok(())
    }

    /// Override the strip sampling interval.
    pub fn set_poll_ms(&mut self, ms: u64) {
        self.poll_ms = ms.max(20);
    }

    pub fn start(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let backend = self.backend.take().context("backend already moved")?;
        let worker_rx = self.worker_rx.take().context("worker already started")?;
        let shared = Arc::clone(&self.shared);

        // Make the effective model explicit, so `/ocw status` and `/model` report
        // what the server would have used anyway.
        if shared.model.lock().unwrap().is_none() {
            if let Some(model) = backend.default_model() {
                eprintln!("[ocw] using the server's default model: {}", model.label());
                *shared.model.lock().unwrap() = Some(model);
            }
        }

        let model = self.config.opencode.model.clone();
        self.worker = Some(thread::spawn(move || {
            worker_loop(backend, worker_rx, shared, model);
        }));
        Ok(())
    }

    pub fn stop(&mut self) {
        self.worker_tx = None;
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    /// Feed a decoded strip payload into the bridge. Used by the self-test.
    #[doc(hidden)]
    pub fn ingest(&mut self, id: u32, payload: &[u8]) {
        self.handle_payload(id, payload);
    }

    #[doc(hidden)]
    pub fn slot_data(&self) -> SlotData {
        self.build_slot_data()
    }

    #[doc(hidden)]
    pub fn session_ids(&self) -> HashMap<u8, String> {
        self.shared.sessions.lock().unwrap().clone()
    }

    /// Backend description, for banners and tests.
    #[doc(hidden)]
    pub fn describe(&self) -> String {
        self.backend_description()
    }

    fn log(&self, message: &str) {
        if self.verbose {
            eprintln!("[ocw] {message}");
        }
    }

    /// Run the read loop for `duration`.
    pub fn run(&mut self, duration: Option<Duration>) -> Result<()> {
        self.start()?;
        let result = self.main_loop(duration);
        self.stop();
        result
    }

    fn main_loop(&mut self, duration: Option<Duration>) -> Result<()> {
        let started = Instant::now();
        let mut errors = 0u32;
        let mut decoded = 0u64;

        eprintln!(
            "[ocw] serving: {} slots under {}, backend={}, strip={}",
            self.bank.slots(),
            self.bank.addons_dir().display(),
            self.backend_description(),
            match &self.strip {
                Some(s) => format!("{}x{} at {},{}", s.width, s.height, s.x, s.y),
                None => "not calibrated (will search the screen)".to_string(),
            }
        );
        eprintln!("[ocw] press Ctrl-C to stop");

        loop {
            if let Some(limit) = duration {
                if started.elapsed() >= limit {
                    break;
                }
            }

            match self.poll_strip() {
                Ok(found) => {
                    errors = 0;
                    if found {
                        decoded += 1;
                    }
                }
                Err(err) => {
                    errors += 1;
                    if errors <= 3 || errors % 100 == 0 {
                        eprintln!("[ocw] capture failed ({errors}): {err}");
                        if errors == 3 {
                            eprintln!(
                                "[ocw] hint: grant Screen Recording permission to your terminal, \
                                 and make sure WoW is windowed or borderless"
                            );
                        }
                    }
                    thread::sleep(Duration::from_millis(500));
                }
            }

            self.maybe_publish();
            thread::sleep(Duration::from_millis(self.poll_ms));
        }

        eprintln!("[ocw] stopped after {decoded} decoded frames");
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

    /// Capture once and try to decode a frame. Returns whether one was read.
    fn poll_strip(&mut self) -> Result<bool> {
        let region = match self.strip {
            Some(settings) => settings.region(),
            None => Region::full(),
        };

        let image = self.capturer.capture(region)?;
        let location = match find_strip(&image) {
            Some(location) => location,
            None => {
                if self.strip.is_some() {
                    // The cached rectangle is stale; look again next time.
                    self.log("strip not found in the cached region; recalibrating");
                    self.strip = None;
                }
                return Ok(false);
            }
        };

        if self.strip.is_none() {
            self.cache_strip(&image, &location);
        }

        let cells = sample_strip(&image, &location);
        let bytes = strip::cells_to_bytes(&cells);
        match strip::decode_frame(&bytes) {
            Ok((id, payload)) => {
                let id = id as u32;
                if self.seen.contains(&id) {
                    return Ok(false);
                }
                self.seen.insert(id);
                if self.seen.len() > 4096 {
                    self.seen.clear();
                }
                let _ = self.bank.signal_ack(id);
                self.log(&format!("strip frame {id}: {} bytes", payload.len()));
                self.handle_payload(id, &payload);
                Ok(true)
            }
            Err(StripError::BadMagic) => Ok(false),
            Err(err) => {
                self.log(&format!("strip seen but rejected: {err}"));
                Ok(false)
            }
        }
    }

    /// Turn a strip location in image pixels into a screen rectangle in points.
    fn cache_strip(&mut self, image: &crate::capture::RgbImage, location: &StripLocation) {
        let scale = match self.capturer.point_scale() {
            Ok(scale) if scale > 0.0 => scale,
            _ => 1.0,
        };
        let to_points = |value: u32| (value as f64 / scale).round() as i32;

        // The full-screen capture starts at the screen origin, so image pixels
        // map directly onto points once divided by the scale.
        let origin_x = to_points(location.x);
        let origin_y = to_points(location.y);
        let settings = StripSettings {
            x: origin_x - REGION_MARGIN,
            y: origin_y - REGION_MARGIN,
            width: to_points(location.width()) as u32 + (REGION_MARGIN as u32) * 2,
            height: to_points(location.height(strip::MAX_ROWS)) as u32 + (REGION_MARGIN as u32) * 2,
        };
        eprintln!(
            "[ocw] strip found: cell {}px, screen {},{} (capturing {}x{} points)",
            location.cell_px, origin_x, origin_y, settings.width, settings.height
        );
        let _ = image;
        self.strip = Some(settings);
        self.config.capture.strip = Some(settings);
    }

    fn handle_payload(&mut self, id: u32, payload: &[u8]) {
        for record in Record::parse_all(payload) {
            if record.is_hello() {
                self.log(&format!("hello from tab {} (session {})", record.tab, record.session));
                self.publish_now();
                continue;
            }
            if record.is_delete() {
                self.log(&format!("tab {} deleted", record.tab));
                self.send(WorkerMsg::CloseSession(record.tab));
                self.jobs.lock().unwrap().retain(|key, _| key.0 != record.tab);
                continue;
            }
            self.on_prompt(id, record);
        }
    }

    fn on_prompt(&mut self, id: u32, record: Record) {
        let key = job_key(record.tab, record.request);
        if self.jobs.lock().unwrap().contains_key(&key) {
            return;
        }

        self.log(&format!(
            "tab {} request {} (strip {id}): {} bytes",
            record.tab,
            record.request,
            record.text.len()
        ));
        self.start_job(key, record);
    }

    fn start_job(&mut self, key: JobKey, record: Record) {
        let status = Arc::new(Mutex::new(JobStatus::default()));
        self.jobs.lock().unwrap().insert(key, Arc::clone(&status));

        if record.starts_new_session() {
            self.send(WorkerMsg::NewSession(record.tab));
        }

        // Local slash-commands are answered without touching the backend.
        if let Some(reply) = self.local_command(record.tab, &record.text) {
            let mut guard = status.lock().unwrap();
            guard.status = ReplyStatus::Done;
            guard.payload = reply;
            drop(guard);
            self.publish_now();
            return;
        }

        let (user_text, game_state) = context::split_game_state(&record.text);
        let git_summary = self.project.as_deref().and_then(git::summarize);
        let ctx = context::compose(
            &self.backend_description(),
            self.project.as_deref(),
            git_summary.as_ref(),
            game_state.as_deref(),
        );

        let job = Job {
            tab: record.tab,
            request: record.request,
            text: user_text,
            context: Some(ctx),
            project: self.project.clone(),
            status,
        };
        self.send(WorkerMsg::Ask(Box::new(job)));
        self.publish_now();
    }

    fn local_command(&mut self, tab: u8, text: &str) -> Option<String> {
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
                self.send(WorkerMsg::Interrupt(tab));
                Some(format!("interrupt requested for tab {tab}"))
            }
            "/new" => {
                self.send(WorkerMsg::NewSession(tab));
                Some(format!("new session started for tab {tab}"))
            }
            "/model" => {
                if arg.is_empty() {
                    return Some(format!("current model: {}", self.backend_description()));
                }
                self.send(WorkerMsg::SetModel {
                    tab,
                    spec: arg.to_string(),
                });
                Some(format!("model set to {arg}"))
            }
            "/status" => {
                let session = self
                    .shared
                    .sessions
                    .lock()
                    .unwrap()
                    .get(&tab)
                    .cloned()
                    .unwrap_or_else(|| "none".to_string());
                let project = self
                    .project
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "none".to_string());
                Some(format!(
                    "backend: opencode\ntab: {tab}\nsession: {session}\nproject: {project}\nmodel: {}",
                    self.backend_description()
                ))
            }
            other => Some(format!("unknown command {other}; try /help")),
        }
    }

    /// Build the slot payload from the current job states.
    fn build_slot_data(&self) -> SlotData {
        let jobs = self.jobs.lock().unwrap();
        let mut replies: Vec<Reply> = jobs
            .iter()
            .map(|(key, status)| {
                let guard = status.lock().unwrap();
                Reply {
                    tab: key.0,
                    request: key.1,
                    status: guard.status,
                    text: guard.payload.clone(),
                    session: guard.session.clone(),
                    denied: Vec::new(),
                }
            })
            .collect();
        replies.sort_by_key(|reply| (reply.tab, reply.request));
        SlotData {
            now: now_epoch(),
            seq: self.slot_seq,
            replies,
        }
    }

    /// Publish slot data now, and raise a readiness signal for anything new.
    fn publish_now(&mut self) {
        self.slot_seq += 1;
        let data = self.build_slot_data();
        let body = crate::slots::render_slot(&data);

        for reply in &data.replies {
            if reply.status != ReplyStatus::Working
                && self.signaled.insert((reply.tab, reply.request))
            {
                if let Err(err) = self.bank.signal_reply(reply.request) {
                    eprintln!("[ocw] could not raise reply signal: {err}");
                }
            }
        }

        if body == self.last_published {
            return;
        }
        match self.bank.publish(&data) {
            Ok(()) => {
                self.log(&format!("published {} replies", data.replies.len()));
                self.last_published = body;
                self.last_publish = Instant::now();
            }
            Err(err) => eprintln!("[ocw] publish failed: {err}"),
        }
    }

    /// Publish on a timer so the addon's status light stays honest.
    fn maybe_publish(&mut self) {
        if self.last_publish.elapsed() >= PUBLISH_INTERVAL {
            self.slot_seq += 1;
            let data = self.build_slot_data();
            let body = crate::slots::render_slot(&data);
            if body != self.last_published {
                if let Err(err) = self.bank.publish(&data) {
                    eprintln!("[ocw] publish failed: {err}");
                } else {
                    self.last_published = body;
                }
            }
            self.last_publish = Instant::now();
        }
    }
}

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
                let session = shared.sessions.lock().unwrap().get(&job.tab).cloned();
                let request = AskRequest {
                    tab: job.tab,
                    request_id: job.request,
                    text: job.text,
                    context: job.context,
                    project: job.project,
                    model: shared.model.lock().unwrap().as_ref().map(|m| m.label()),
                    session_id: session,
                };

                match backend.ask(&request) {
                    Ok(response) => {
                        let mut guard = status.lock().unwrap();
                        if let Some(session) = &response.session_id {
                            shared
                                .sessions
                                .lock()
                                .unwrap()
                                .insert(job.tab, session.clone());
                            guard.session = session.clone();
                        }
                        guard.status = ReplyStatus::Done;
                        guard.payload = response.text;
                    }
                    Err(err) => {
                        let mut guard = status.lock().unwrap();
                        guard.status = ReplyStatus::Error;
                        guard.payload = format!("error: {err}");
                    }
                }
            }
            WorkerMsg::Interrupt(tab) => {
                if let Some(session) = shared.sessions.lock().unwrap().get(&tab).cloned() {
                    let _ = backend.interrupt(&session);
                }
            }
            WorkerMsg::SetModel { tab, spec } => {
                let parsed = ModelRef::parse(&spec);
                *shared.model.lock().unwrap() = parsed.clone();
                // Apply it to the session that asked, so the change is immediate
                // rather than only affecting the next new session.
                if let (Some(model), Some(session)) = (
                    parsed,
                    shared.sessions.lock().unwrap().get(&tab).cloned(),
                ) {
                    if let Err(err) = backend.switch_model(&session, &model) {
                        eprintln!("[ocw] could not switch model for tab {tab}: {err}");
                    }
                }
            }
            WorkerMsg::NewSession(tab) | WorkerMsg::CloseSession(tab) => {
                shared.sessions.lock().unwrap().remove(&tab);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fields: &[&str]) -> Option<Record> {
        let owned: Vec<Vec<u8>> = fields.iter().map(|f| f.as_bytes().to_vec()).collect();
        Record::from_fields(&owned)
    }

    #[test]
    fn parses_a_plain_record() {
        let parsed = record(&["sess", "2", "7", "", "", "Chat", "hello there"]).unwrap();
        assert_eq!(parsed.session, "sess");
        assert_eq!(parsed.tab, 2);
        assert_eq!(parsed.request, 7);
        assert_eq!(parsed.text, "hello there");
        assert!(parsed.context.is_none());
    }

    #[test]
    fn parses_a_context_record() {
        let parsed = record(&["s", "1", "3", "", "c", "Chat", "zone: Elwynn", "the prompt"]).unwrap();
        assert_eq!(parsed.context.as_deref(), Some("zone: Elwynn"));
        assert_eq!(parsed.text, "the prompt");
    }

    #[test]
    fn recognises_flags() {
        let hello = record(&["s", "1", "1", "", "h", "Chat", "x"]).unwrap();
        assert!(hello.is_hello() && !hello.is_delete());
        let deleted = record(&["s", "1", "1", "", "d", "Chat", ""]).unwrap();
        assert!(deleted.is_delete());
        let fresh = record(&["s", "1", "1", "", "n;c", "Chat", "ctx", "x"]).unwrap();
        assert!(fresh.starts_new_session());
    }

    #[test]
    fn rejects_short_records() {
        assert!(record(&["s", "1"]).is_none());
    }

    #[test]
    fn job_keys_are_unique() {
        assert_ne!(job_key(1, 1), job_key(1, 2));
        assert_ne!(job_key(1, 1), job_key(2, 1));
    }
}
