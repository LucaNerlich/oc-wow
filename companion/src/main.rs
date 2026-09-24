//! `ocw` - the OCWow companion binary.
//!
//! Subcommands:
//!   * `install`  - write the addon and build the font bank
//!   * `probe`    - capture once and locate the pixel strip (calibration)
//!   * `run`      - serve the bridge (real OpenCode server or `--mock`)
//!   * `ping`     - check the OpenCode server
//!   * `models`   - list available models
//!   * `projects` - list known projects
//!   * `dump`     - decode the reply stored in a font slot
//!   * `font`     - build a reply font from text (debug)
//!   * `paths`    - show resolved paths

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use ocw::app::App;
use ocw::backend::{Backend, Mock, OpenCode};
use ocw::capture::{Capturer, Region};
use ocw::config::{self, Config};
use ocw::fonts::{Bank, DEFAULT_SLOTS};
use ocw::protocol::frames::{
    reply_state, ControlFrame, PromptFrame, ReplyFrame, PROTOCOL_VERSION,
};
use ocw::protocol::pixel;

/// Addon sources embedded so the binary is self-contained on any platform.
const ADDON_FILES: &[(&str, &str)] = &[
    ("OCWow.toc", include_str!("../../addon/OCWow/OCWow.toc")),
    ("Protocol.lua", include_str!("../../addon/OCWow/Protocol.lua")),
    ("Native.lua", include_str!("../../addon/OCWow/Native.lua")),
    ("Context.lua", include_str!("../../addon/OCWow/Context.lua")),
    ("UI.lua", include_str!("../../addon/OCWow/UI.lua")),
    ("Main.lua", include_str!("../../addon/OCWow/Main.lua")),
];

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
    /// Write the addon files and create the font bank.
    Install(InstallArgs),
    /// Capture once and locate the pixel strip.
    Probe(ProbeArgs),
    /// Run the bridge.
    Run(RunArgs),
    /// Check the OpenCode server.
    Ping(ServerArgs),
    /// List available models.
    Models(ServerArgs),
    /// List known projects.
    Projects(ServerArgs),
    /// Decode the reply stored in a font slot.
    Dump(DumpArgs),
    /// Build a reply font from text (debugging).
    Font(FontArgs),
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
    /// Number of font-bank slots.
    #[arg(long, default_value_t = DEFAULT_SLOTS)]
    slots: u16,
    /// Rewrite existing slot files (unsafe while the game is running).
    #[arg(long)]
    force: bool,
    /// Only write the Lua addon files, not the font bank.
    #[arg(long)]
    no_bank: bool,
}

#[derive(Args, Debug)]
struct ProbeArgs {
    /// Crop as `x,y,w,h`, or `full` for the whole screen.
    #[arg(long)]
    crop: Option<String>,
    /// Cell edge length in pixels (0 = auto-detect).
    #[arg(long, default_value_t = 0)]
    cell: u32,
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
    /// Crop as `x,y,w,h`, or `full`.
    #[arg(long)]
    crop: Option<String>,
    /// Cell edge length in pixels (0 = auto-detect).
    #[arg(long)]
    cell: Option<u32>,
    /// Capture command template.
    #[arg(long)]
    capture_cmd: Option<String>,
    /// Stop after this many seconds (default: run until Ctrl-C).
    #[arg(long)]
    duration: Option<u64>,
    /// Strip sample interval in milliseconds.
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

#[derive(Args, Debug)]
struct DumpArgs {
    #[arg(long)]
    slot: u16,
    #[arg(long)]
    addon_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct FontArgs {
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    text: String,
    /// Write a baseline (all-zero) font instead of encoding text.
    #[arg(long)]
    baseline: bool,
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
        Command::Probe(args) => cmd_probe(&mut cfg, args),
        Command::Run(args) => cmd_run(&mut cfg, args, cli.verbose),
        Command::Ping(args) => cmd_ping(&cfg, args),
        Command::Models(args) => cmd_models(&cfg, args),
        Command::Projects(args) => cmd_projects(&cfg, args),
        Command::Dump(args) => cmd_dump(&cfg, args),
        Command::Font(args) => cmd_font(args),
        Command::Selftest(args) => cmd_selftest(&mut cfg, args),
        Command::Paths => cmd_paths(&cfg, &config_path),
    }
}

fn load_config(path: &Option<PathBuf>) -> Result<(Config, PathBuf)> {
    let config_path = path.clone().unwrap_or_else(config::default_config_path);
    let cfg = Config::load(&config_path)?;
    Ok((cfg, config_path))
}

fn parse_region(spec: &str) -> Result<Region> {
    if spec.eq_ignore_ascii_case("full") {
        return Ok(Region::full_screen());
    }
    let parts: Vec<&str> = spec.split(',').map(|s| s.trim()).collect();
    if parts.len() != 4 {
        bail!("crop must be `x,y,w,h` or `full`, got {spec:?}");
    }
    Ok(Region {
        x: parts[0].parse().context("crop x")?,
        y: parts[1].parse().context("crop y")?,
        width: parts[2].parse().context("crop width")?,
        height: parts[3].parse().context("crop height")?,
    })
}

fn apply_capture_overrides(
    cfg: &mut Config,
    crop: Option<&String>,
    cell: Option<u32>,
    capture_cmd: Option<&String>,
) -> Result<()> {
    if let Some(spec) = crop {
        let region = parse_region(spec)?;
        cfg.capture.x = region.x;
        cfg.capture.y = region.y;
        cfg.capture.width = region.width;
        cfg.capture.height = region.height;
    }
    if let Some(cell) = cell {
        cfg.capture.cell_px = cell;
    }
    if let Some(cmd) = capture_cmd {
        cfg.capture.command = Some(cmd.clone());
    }
    Ok(())
}

/// Resolve the server URL and password, preferring the live service registration.
fn resolve_server(
    cfg: &Config,
    base_url: &Option<String>,
    password: &Option<String>,
) -> (String, Option<String>) {
    if let Some(url) = base_url {
        return (url.clone(), password.clone().or_else(|| cfg.opencode.password.clone()));
    }
    if let Some((url, pw)) = config::read_service_registration() {
        return (url, password.clone().or(Some(pw)));
    }
    (cfg.opencode.base_url.clone(), password.clone().or_else(|| cfg.opencode.password.clone()))
}

fn cmd_install(cfg: &mut Config, config_path: &PathBuf, args: InstallArgs) -> Result<()> {
    let addon_dir = args.addon_dir.clone().unwrap_or_else(|| cfg.addon_dir.clone());
    std::fs::create_dir_all(&addon_dir)
        .with_context(|| format!("creating addon directory {}", addon_dir.display()))?;

    for (name, contents) in ADDON_FILES {
        let path = addon_dir.join(name);
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    }
    println!("wrote {} addon files to {}", ADDON_FILES.len(), addon_dir.display());

    if !args.no_bank {
        let bank = Bank::new(addon_dir.join("Fonts"), args.slots);
        let report = bank.install(args.force)?;
        println!(
            "font bank: {} slots in {} ({} hard-linked, {} copied)",
            report.slots,
            report.dir.display(),
            report.hard_linked,
            report.copied
        );
    }

    cfg.addon_dir = addon_dir;
    cfg.bank_slots = args.slots;
    cfg.save(config_path)?;
    println!("configuration saved to {}", config_path.display());
    println!("\nNext: /reload in game (or restart the client if new assets are not discovered),");
    println!("then run `ocw probe` and `ocw run`.");
    Ok(())
}

fn cmd_probe(cfg: &mut Config, args: ProbeArgs) -> Result<()> {
    apply_capture_overrides(
        cfg,
        args.crop.as_ref(),
        if args.cell == 0 { None } else { Some(args.cell) },
        args.capture_cmd.as_ref(),
    )?;

    let region = cfg.capture.region();
    println!("capturing region {:?}...", region);
    let capturer = Capturer::new(region, cfg.capture.command.clone(), cfg.capture.cell_px);
    let image = capturer.capture()?;
    println!("captured {}x{} pixels", image.width, image.height);

    let gray = image.as_gray();
    match pixel::find_strip(&gray) {
        Some((cell, dx, dy, bytes)) => {
            let kind = match bytes[2] {
                1 => "prompt",
                2 => "control",
                other => {
                    let _ = other;
                    "unknown"
                }
            };
            println!("found strip: cell={cell}px, offset=({dx}, {dy}), frame type={kind}");
            let width = pixel::STRIP_COLS as u32 * cell;
            let height = pixel::STRIP_ROWS as u32 * cell;
            println!(
                "suggested: --crop {},{},{},{}",
                region.x + dx as i32,
                region.y + dy as i32,
                width,
                height
            );
            println!("save it with: ocw run --crop {},{},{},{} --cell 0",
                region.x + dx as i32,
                region.y + dy as i32,
                width,
                height);
        }
        None => {
            println!("no strip found.");
            println!(" - in game, run `/ocw calibrate` to show a fixed frame");
            println!(" - make sure the WoW window is visible (windowed or borderless)");
            println!(" - grant Screen Recording permission to your terminal");
            println!(" - try a wider probe: ocw probe --crop full");
        }
    }
    Ok(())
}

fn cmd_run(cfg: &mut Config, args: RunArgs, verbose: bool) -> Result<()> {
    apply_capture_overrides(
        cfg,
        args.crop.as_ref(),
        args.cell,
        args.capture_cmd.as_ref(),
    )?;
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
    app.check_bank()?;
    if let Some(ms) = args.poll_ms {
        app.set_poll_ms(ms);
    }
    app.run(args.duration.map(Duration::from_secs))
}

fn cmd_ping(cfg: &Config, args: ServerArgs) -> Result<()> {
    let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
    let client = ocw::http::Client::new(&url, password)?;
    let health = client.get_json("/api/health")?;
    println!("{url}");
    println!("{}", serde_json::to_string_pretty(&health)?);
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
    let backend = opencode_for(cfg, &args)?;
    let models = backend.list_models()?;
    println!("{} models:", models.len());
    for model in models {
        println!("  {}", model.label());
    }
    Ok(())
}

fn cmd_projects(cfg: &Config, args: ServerArgs) -> Result<()> {
    let backend = opencode_for(cfg, &args)?;
    let projects = backend.list_projects()?;
    println!("{} projects:", projects.len());
    for project in projects {
        let canonical = project.get("canonical").and_then(|v| v.as_str()).unwrap_or("?");
        let vcs = project.get("vcs").and_then(|v| v.as_str()).unwrap_or("");
        println!("  {canonical} {vcs}");
    }
    Ok(())
}

fn cmd_dump(cfg: &Config, args: DumpArgs) -> Result<()> {
    let addon_dir = args.addon_dir.clone().unwrap_or_else(|| cfg.addon_dir.clone());
    let bank = Bank::new(addon_dir.join("Fonts"), cfg.bank_slots);
    let bytes = bank.read_reply(args.slot)?;
    match ReplyFrame::decode(&bytes) {
        Ok(frame) => {
            println!("slot {} holds a valid packet:", args.slot);
            println!("  state: {}", ocw::protocol::frames::reply_state::name(frame.state));
            println!("  request: {} (session {})", frame.request_id, frame.ui_session);
            println!("  fragment: {}/{}", frame.fragment_index, frame.fragment_count);
            println!("  revision: {}", frame.revision);
            println!("  payload: {}", String::from_utf8_lossy(&frame.payload));
        }
        Err(err) => println!("slot {} has no valid packet ({err})", args.slot),
    }
    Ok(())
}

fn cmd_font(args: FontArgs) -> Result<()> {
    if args.baseline {
        std::fs::write(&args.out, ocw::fonts::build_baseline_font())?;
        println!("wrote baseline font to {}", args.out.display());
        return Ok(());
    }
    let mut packet = [0u8; 512];
    let frame = ReplyFrame {
        state: ocw::protocol::frames::reply_state::DONE,
        ui_session: 1,
        request_id: 1,
        fragment_index: 1,
        fragment_count: 1,
        revision: 1,
        slot: 1,
        flags: 0,
        payload: args.text.as_bytes().to_vec(),
    };
    let encoded = frame.encode();
    packet.copy_from_slice(&encoded);
    let font = ocw::fonts::build_reply_font(&packet);
    std::fs::write(&args.out, font)?;
    println!(
        "wrote reply font (protocol v{PROTOCOL_VERSION}) to {}",
        args.out.display()
    );
    Ok(())
}

/// Drive the bridge end-to-end in-process: encode a prompt the way the addon
/// would, ingest it, then read the reply back out of the font bank.
fn cmd_selftest(cfg: &mut Config, args: SelftestArgs) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("ocw-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let addon_dir = dir.join("OCWow");
    let slots = 64u16;

    let bank = Bank::new(addon_dir.join("Fonts"), slots);
    bank.install(true)?;

    cfg.addon_dir = addon_dir.clone();
    cfg.bank_slots = slots;
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

    let ui_session = 4242u16;
    let request_id = 1u32;
    let prompt = if args.live {
        "Reply with exactly the single word: pong"
    } else {
        "selftest: hello from the fake addon"
    };
    println!("backend: {}", app.describe());
    println!("prompt:  {prompt}");

    let frame = PromptFrame {
        ui_session,
        request_id,
        fragment_index: 0,
        fragment_count: 1,
        flags: 0,
        payload: prompt.as_bytes().to_vec(),
    };
    app.ingest_frame(&frame.encode());

    let attempts = if args.live { 180 } else { 16 };
    let pause = if args.live { 500 } else { 40 };

    let mut slot = 1u16;
    let mut reply: Option<String> = None;
    for _ in 0..attempts {
        std::thread::sleep(Duration::from_millis(pause));
        let control = ControlFrame {
            ui_session,
            request_id,
            requested_fragment: 1,
            slot,
            deadline_ms: 1000,
            state: reply_state::WORKING,
            active: true,
            last_attempted_slot: slot.saturating_sub(1),
        };
        app.ingest_frame(&control.encode());

        let bytes = bank.read_reply(slot)?;
        if let Ok(packet) = ReplyFrame::decode(&bytes) {
            if packet.request_id == request_id && packet.slot == slot {
                println!(
                    "slot {slot}: state={} payload={:?}",
                    reply_state::name(packet.state),
                    String::from_utf8_lossy(&packet.payload)
                );
                if packet.state == reply_state::DONE {
                    reply = Some(String::from_utf8_lossy(&packet.payload).into_owned());
                    break;
                }
            }
        }
        slot += 1;
        if slot > slots {
            slot = 1;
        }
    }

    let session = app.session_id();
    app.stop();

    // Clean up a live session so the test leaves no trace, and verify that it
    // was actually scoped to the requested project directory.
    let mut project_ok = true;
    if args.live {
        if let Some(session) = &session {
            let (url, password) = resolve_server(cfg, &args.base_url, &args.password);
            if let Ok(client) = OpenCode::new(&url, password, None, None, 60) {
                if let Ok(info) = client.get_session(session) {
                    let directory = info
                        .get("location")
                        .and_then(|l| l.get("directory"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("session directory: {directory}");
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

    let expected = if args.live { "pong" } else { "hello from the fake addon" };
    match reply {
        Some(text) if text.to_lowercase().contains(expected) && project_ok => {
            println!("SELFTEST PASSED: reply round-tripped through the font bank");
            Ok(())
        }
        Some(text) if !project_ok => bail!("SELFTEST FAILED: session was not scoped to the project"),
        Some(text) => bail!("SELFTEST FAILED: unexpected reply {text:?}"),
        None => bail!("SELFTEST FAILED: no reply arrived"),
    }
}

fn cmd_paths(cfg: &Config, config_path: &PathBuf) -> Result<()> {    println!("config:     {}", config_path.display());
    println!("state:      {}", ocw::state::default_state_path().display());
    println!("addon dir:  {}", cfg.addon_dir.display());
    println!("font bank:  {}", cfg.bank().dir().display());
    println!("bank slots: {}", cfg.bank_slots);
    println!("crop:       {:?}", cfg.capture.region());
    if cfg.capture.cell_px == 0 {
        println!("cell px:    auto");
    } else {
        println!("cell px:    {}", cfg.capture.cell_px);
    }
    match config::read_service_registration() {
        Some((url, _)) => println!("service:    {url}"),
        None => println!("service:    (no registration found)"),
    }
    match ocw::state::find_wow_pid() {
        Some(pid) => println!("wow pid:    {pid}"),
        None => println!("wow pid:    (not running)"),
    }
    println!(
        "addon installed: {}",
        if cfg.bank().is_installed() { "yes" } else { "no" }
    );
    Ok(())
}
