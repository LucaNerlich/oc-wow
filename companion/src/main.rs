//! `ocw` - the OCWow companion binary.
//!
//! Subcommands:
//!   * `install`  - write the addon and create the slot bank
//!   * `probe`    - find the pixel strip on screen and remember where it is
//!   * `run`      - serve the bridge (real OpenCode server or `--mock`)
//!   * `ping` / `models` / `projects` - inspect the OpenCode server
//!   * `selftest` - in-process bridge round-trip
//!   * `paths`    - show resolved paths

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use ocw::app::App;
use ocw::backend::{Backend, Mock, OpenCode};
use ocw::capture::{find_strip, Capturer};
use ocw::config::{self, Config, StripSettings};
use ocw::protocol::strip::{self, CELLS_PER_ROW, MAX_ROWS};
use ocw::slots::{ReplyStatus, SlotBank};

/// Addon sources embedded so the binary is self-contained on any platform.
const ADDON_FILE_NAMES: &[&str] = &[
    "OCWow.toc",
    "Codec.lua",
    "Context.lua",
    "UI.lua",
    "Main.lua",
];

const ADDON_FILES: &[(&str, &str)] = &[
    ("OCWow.toc", include_str!("../../addon/OCWow/OCWow.toc")),
    ("Codec.lua", include_str!("../../addon/OCWow/Codec.lua")),
    ("Context.lua", include_str!("../../addon/OCWow/Context.lua")),
    ("UI.lua", include_str!("../../addon/OCWow/UI.lua")),
    ("Main.lua", include_str!("../../addon/OCWow/Main.lua")),
];

/// Load the addon sources from a directory, or fall back to the embedded copies.
fn load_addon_sources(from: Option<&Path>) -> Result<Vec<(String, String)>> {
    match from {
        None => Ok(ADDON_FILES
            .iter()
            .map(|(name, contents)| ((*name).to_string(), (*contents).to_string()))
            .collect()),
        Some(dir) => {
            let mut sources = Vec::with_capacity(ADDON_FILE_NAMES.len());
            for name in ADDON_FILE_NAMES {
                let path = dir.join(name);
                let contents = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                sources.push(((*name).to_string(), contents));
            }
            Ok(sources)
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "ocw",
    version,
    about = "OpenCode companion for the World of Warcraft OCWow addon",
    long_about = None
)]
struct Cli {
    /// Configuration file (defaults to the platform config directory).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Print extra diagnostics.
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write the addon files and create the load-on-demand slot bank.
    Install(InstallArgs),
    /// Find the pixel strip on screen and remember where it is.
    Probe(ProbeArgs),
    /// Run the bridge.
    Run(RunArgs),
    /// Check the OpenCode server.
    Ping(ServerArgs),
    /// List available models.
    Models(ServerArgs),
    /// List known projects.
    Projects(ServerArgs),
    /// Run an in-process bridge round-trip without a game client.
    Selftest(SelftestArgs),
    /// Show resolved configuration paths.
    Paths,
}

#[derive(Args, Debug)]
struct InstallArgs {
    /// Addon directory to install into.
    #[arg(long)]
    addon_dir: Option<PathBuf>,
    /// Number of load-on-demand slots.
    #[arg(long, default_value_t = ocw::slots::DEFAULT_SLOTS)]
    slots: u16,
    /// `## Interface:` version for generated slots.
    #[arg(long, default_value_t = config::DEFAULT_INTERFACE)]
    interface: u32,
    /// Rewrite existing slot files.
    #[arg(long)]
    force: bool,
    /// Only write the addon files, not the slot bank.
    #[arg(long)]
    no_slots: bool,
    /// Read the addon Lua/TOC from this directory instead of the embedded copies.
    #[arg(long)]
    from: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ProbeArgs {
    /// Show every candidate, not just the first match.
    #[arg(long)]
    all: bool,
    /// Capture command template (see docs/setup.md).
    #[arg(long)]
    capture_cmd: Option<String>,
}

#[derive(Args, Debug)]
struct RunArgs {
    /// Use the local echo backend instead of OpenCode.
    #[arg(long)]
    mock: bool,
    /// Project directory for new sessions.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Model reference, `provider/model`.
    #[arg(long)]
    model: Option<String>,
    /// OpenCode server base URL.
    #[arg(long)]
    base_url: Option<String>,
    /// OpenCode service password.
    #[arg(long)]
    password: Option<String>,
    /// Capture command template.
    #[arg(long)]
    capture_cmd: Option<String>,
    /// Stop after this many seconds (default: run until Ctrl-C).
    #[arg(long)]
    duration: Option<u64>,
    /// Strip sampling interval in milliseconds.
    #[arg(long)]
    poll_ms: Option<u64>,
}

#[derive(Args, Debug)]
struct ServerArgs {
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long)]
    project: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct SelftestArgs {
    /// Use the live OpenCode backend instead of the local mock.
    #[arg(long)]
    live: bool,
    /// Model reference for the live test.
    #[arg(long)]
    model: Option<String>,
    /// Project directory for the live test.
    #[arg(long)]
    project: Option<PathBuf>,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long)]
    password: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let (mut cfg, config_path) = load_config(&cli.config)?;

    match cli.command {
        Command::Install(args) => cmd_install(&mut cfg, &config_path, args),
        Command::Probe(args) => cmd_probe(&mut cfg, &config_path, args),
        Command::Run(args) => cmd_run(&mut cfg, args, cli.verbose),
        Command::Ping(args) => cmd_ping(&cfg, args),
        Command::Models(args) => cmd_models(&cfg, args),
        Command::Projects(args) => cmd_projects(&cfg, args),
        Command::Selftest(args) => cmd_selftest(&mut cfg, args),
        Command::Paths => cmd_paths(&cfg, &config_path),
    }
}

fn load_config(path: &Option<PathBuf>) -> Result<(Config, PathBuf)> {
    let config_path = path.clone().unwrap_or_else(config::default_config_path);
    let cfg = Config::load(&config_path)?;
    Ok((cfg, config_path))
}

fn resolve_server(
    cfg: &Config,
    base_url: &Option<String>,
    password: &Option<String>,
) -> (String, Option<String>) {
    if let Some(url) = base_url {
        return (
            url.clone(),
            password.clone().or_else(|| cfg.opencode.password.clone()),
        );
    }
    if let Some((url, pw)) = config::read_service_registration() {
        return (url, password.clone().or(Some(pw)));
    }
    (
        cfg.opencode.base_url.clone(),
        password.clone().or_else(|| cfg.opencode.password.clone()),
    )
}

fn cmd_install(cfg: &mut Config, config_path: &PathBuf, args: InstallArgs) -> Result<()> {
    let addon_dir = args
        .addon_dir
        .clone()
        .unwrap_or_else(|| cfg.addon_dir.clone());
    std::fs::create_dir_all(&addon_dir)
        .with_context(|| format!("creating addon directory {}", addon_dir.display()))?;

    let sources = load_addon_sources(args.from.as_deref())?;
    for (name, contents) in &sources {
        let path = addon_dir.join(name);
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    }
    let origin = match &args.from {
        Some(dir) => format!("from {}", dir.display()),
        None => "from the embedded copies".to_string(),
    };
    println!(
        "wrote {} addon files to {} ({origin})",
        sources.len(),
        addon_dir.display()
    );

    if args.from.is_none() {
        let local = PathBuf::from("addon/OCWow");
        if local.join("OCWow.toc").is_file() {
            println!(
                "note: ./addon/OCWow exists; use --from addon/OCWow to deploy local \
                 edits without rebuilding"
            );
        }
    }

    if !args.no_slots {
        let bank = SlotBank::new(addon_dir.clone(), args.slots);
        let report = bank.install(args.interface, args.force)?;
        println!(
            "slot bank: {} slots in {} ({} created, {} signal files)",
            report.slots,
            report.addons_dir.display(),
            report.created,
            report.signals
        );
        println!(
            "note: slots are separate addons in the AddOns list; leave them disabled. \
             Restart WoW after installing so the client discovers them."
        );
    }

    cfg.addon_dir = addon_dir;
    cfg.slots = args.slots;
    cfg.interface = args.interface;
    cfg.save(config_path)?;
    println!("configuration saved to {}", config_path.display());
    println!("\nNext: fully restart WoW, then run `ocw probe` while the strip is visible.");
    Ok(())
}

fn cmd_probe(cfg: &mut Config, config_path: &PathBuf, args: ProbeArgs) -> Result<()> {
    let capturer = Capturer::new(args.capture_cmd.clone().or(cfg.capture.command.clone()));
    println!("capturing the whole screen...");
    let image = capturer.capture_full()?;
    println!("captured {}x{} pixels", image.width, image.height);

    match find_strip(&image) {
        Some(location) => {
            let scale = capturer.point_scale().unwrap_or(1.0);
            let to_points = |value: u32| (value as f64 / scale).round() as i32;
            let settings = StripSettings {
                x: to_points(location.x) - 24,
                y: to_points(location.y) - 24,
                width: to_points(location.width()) as u32 + 48,
                height: to_points(location.height(MAX_ROWS)) as u32 + 48,
            };
            println!(
                "strip found: cell {}px at image ({}, {}), capture scale {scale}",
                location.cell_px, location.x, location.y
            );
            println!(
                "screen rectangle: {}x{} points at {},{}",
                settings.width, settings.height, settings.x, settings.y
            );
            cfg.capture.strip = Some(settings);
            cfg.save(config_path)?;
            println!("saved to {}", config_path.display());
        }
        None => {
            println!("no strip found.");
            println!(" - in game, open the panel with /ocw and send anything, or run /ocw test");
            println!(" - make sure WoW is windowed or borderless and the window is on screen");
            println!(" - grant Screen Recording permission to your terminal");
            println!(
                " - the strip is {}x{} cells of {}px in the top-left of the game window",
                CELLS_PER_ROW,
                MAX_ROWS,
                strip::CELL_PX
            );
            let _ = args.all;
        }
    }
    Ok(())
}

fn cmd_run(cfg: &mut Config, args: RunArgs, verbose: bool) -> Result<()> {
    if let Some(command) = args.capture_cmd.clone() {
        cfg.capture.command = Some(command);
    }
    if let Some(project) = args.project.clone() {
        cfg.opencode.project = Some(project);
    }
    if let Some(model) = args.model.clone() {
        cfg.opencode.model = Some(model);
    }

    let backend = if args.mock {
        Backend::Mock(Mock::new())
    } else {
        let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
        println!("connecting to OpenCode at {url}");
        Backend::OpenCode(Box::new(OpenCode::new(
            &url,
            password,
            cfg.opencode.project.clone(),
            cfg.opencode.model.clone(),
            cfg.opencode.timeout_secs,
        )?))
    };

    let mut app = App::new(cfg.clone(), backend, verbose)?;
    app.check_slots()?;
    if let Some(ms) = args.poll_ms {
        app.set_poll_ms(ms);
    }
    app.run(args.duration.map(Duration::from_secs))
}

fn cmd_ping(cfg: &Config, args: ServerArgs) -> Result<()> {
    let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
    let client = ocw::http::Client::new(&url, password)?;
    println!("{url}");
    println!(
        "{}",
        serde_json::to_string_pretty(&client.get_json("/api/health")?)?
    );
    Ok(())
}

fn opencode_for(cfg: &Config, args: &ServerArgs) -> Result<OpenCode> {
    let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
    OpenCode::new(
        &url,
        password,
        args.project.clone().or_else(|| cfg.opencode.project.clone()),
        cfg.opencode.model.clone(),
        cfg.opencode.timeout_secs,
    )
}

fn cmd_models(cfg: &Config, args: ServerArgs) -> Result<()> {
    let models = opencode_for(cfg, &args)?.list_models()?;
    println!("{} models:", models.len());
    for model in models {
        println!("  {}", model.label());
    }
    Ok(())
}

fn cmd_projects(cfg: &Config, args: ServerArgs) -> Result<()> {
    let projects = opencode_for(cfg, &args)?.list_projects()?;
    println!("{} projects:", projects.len());
    for project in projects {
        let canonical = project.get("canonical").and_then(|v| v.as_str()).unwrap_or("?");
        let vcs = project.get("vcs").and_then(|v| v.as_str()).unwrap_or("");
        println!("  {canonical} {vcs}");
    }
    Ok(())
}

/// Build the strip payload the addon would send for one message.
fn record_payload(session: &str, tab: u8, request: u32, flags: &str, text: &str) -> Vec<u8> {
    [
        session.to_string(),
        tab.to_string(),
        request.to_string(),
        String::new(),
        flags.to_string(),
        format!("Chat {tab}"),
        text.to_string(),
    ]
    .join("\u{1f}")
    .into_bytes()
}

/// Drive the bridge end-to-end in-process: feed strip payloads the way the
/// addon would, then read each tab's reply out of the slot bank.
fn cmd_selftest(cfg: &mut Config, args: SelftestArgs) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("ocw-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let addon_dir = dir.join("AddOns").join("OCWow");
    std::fs::create_dir_all(&addon_dir)?;
    let slots = 8u16;

    let bank = SlotBank::new(addon_dir.clone(), slots);
    bank.install(16001, true)?;

    cfg.addon_dir = addon_dir;
    cfg.slots = slots;
    if let Some(model) = args.model.clone() {
        cfg.opencode.model = Some(model);
    }
    if let Some(project) = args.project.clone() {
        cfg.opencode.project = Some(project);
    }

    let backend = if args.live {
        let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
        println!("live backend at {url}");
        Backend::OpenCode(Box::new(OpenCode::new(
            &url,
            password,
            cfg.opencode.project.clone(),
            cfg.opencode.model.clone(),
            cfg.opencode.timeout_secs,
        )?))
    } else {
        Backend::Mock(Mock::new())
    };

    let mut app = App::new(cfg.clone(), backend, false)?;
    app.start()?;
    println!("backend: {}", app.describe());

    let tabs: Vec<u8> = if args.live { vec![1] } else { vec![1, 2] };
    for tab in &tabs {
        let text = if args.live {
            "Reply with exactly the single word: pong".to_string()
        } else {
            format!("selftest tab {tab}")
        };
        println!("tab {tab} prompt: {text}");
        // The addon sends a record per message; the strip id increments.
        app.ingest(*tab as u32, &record_payload("selftest", *tab, 1, "", &text));
    }

    let attempts = if args.live { 180 } else { 40 };
    let pause = if args.live { 500 } else { 50 };

    let mut replies: HashMap<u8, String> = HashMap::new();
    for _ in 0..attempts {
        std::thread::sleep(Duration::from_millis(pause));
        for reply in app.slot_data().replies {
            if reply.status != ReplyStatus::Working {
                replies
                    .entry(reply.tab)
                    .or_insert_with(|| reply.text.clone());
            }
        }
        if replies.len() == tabs.len() {
            break;
        }
    }

    let sessions = app.session_ids();
    app.stop();

    let mut project_ok = true;
    if args.live {
        let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
        if let Ok(client) = OpenCode::new(&url, password, None, None, 60) {
            for (tab, session) in &sessions {
                if let Ok(info) = client.get_session(session) {
                    let directory = info
                        .get("location")
                        .and_then(|l| l.get("directory"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("tab {tab} session directory: {directory}");
                    if let Some(expected) = &cfg.opencode.project {
                        let expected = expected.display().to_string();
                        if directory != expected {
                            project_ok = false;
                            eprintln!(
                                "SELFTEST FAILED: session ran in {directory}, expected {expected}"
                            );
                        }
                    }
                }
                let _ = client.delete_session(session);
                println!("deleted test session {session}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    if !project_ok {
        bail!("SELFTEST FAILED: a session was not scoped to the requested project");
    }

    let mut failed = false;
    for tab in &tabs {
        let expected = if args.live {
            "pong".to_string()
        } else {
            format!("tab {tab}")
        };
        match replies.get(tab) {
            Some(text) if text.to_lowercase().contains(&expected.to_lowercase()) => {
                println!("tab {tab} reply: {text:?}");
            }
            Some(text) => {
                failed = true;
                eprintln!("SELFTEST FAILED: tab {tab} reply {text:?} lacks {expected:?}");
            }
            None => {
                failed = true;
                eprintln!("SELFTEST FAILED: tab {tab} got no reply");
            }
        }
    }
    if failed {
        bail!("SELFTEST FAILED");
    }

    println!(
        "SELFTEST PASSED: {} tab(s) round-tripped through the slot bank",
        tabs.len()
    );
    Ok(())
}

fn cmd_paths(cfg: &Config, config_path: &PathBuf) -> Result<()> {
    println!("config:     {}", config_path.display());
    println!("state:      {}", ocw::state::default_state_path().display());
    println!("addon dir:  {}", cfg.addon_dir.display());
    println!("addons dir: {}", cfg.slot_bank().addons_dir().display());
    println!("slots:      {}", cfg.slots);
    match cfg.capture.strip {
        Some(settings) => println!(
            "strip:      {}x{} points at {},{}",
            settings.width, settings.height, settings.x, settings.y
        ),
        None => println!("strip:      not calibrated (run `ocw probe`)"),
    }
    println!(
        "capture:    {}",
        cfg.capture.command.clone().unwrap_or_else(|| "(platform default)".to_string())
    );
    match config::read_service_registration() {
        Some((url, _)) => println!("service:    {url}"),
        None => println!("service:    (no registration found)"),
    }
    match ocw::state::find_wow_pid() {
        Some(pid) => println!("wow pid:    {pid}"),
        None => println!("wow pid:    (not running)"),
    }
    println!(
        "slots installed: {}",
        if cfg.slot_bank().is_installed() { "yes" } else { "no" }
    );
    Ok(())
}
