# OCWow

Drive [OpenCode](https://opencode.ai) from inside World of Warcraft.

OCWow is a WoW addon plus a small desktop companion. You type a prompt into an
in-game panel, keep playing, and the answer comes back into the game as normal
text. It works for WoW questions ("what's my current quest?", "is this item an
upgrade?") and for real coding work in any project on your machine.

```
  WoW addon (Lua, sandboxed)              Companion (Rust, cross-platform)        OpenCode
  ──────────────────────────              ────────────────────────────────        ────────
  tabs: one session each                  pixel-strip decoder  (screen capture) ─┐
   • prompt box                           slot-bank writer     (files)         ─┘ transport
   • game-state context tiers             OpenCode HTTP client ────────────────► serve (V2 API)
   • /model /new /stop commands           git / VCS reporting
   • status + progress                    context assembler
```

## How the two channels work

WoW addons are sandboxed: no sockets, no file reads at runtime. OCWow moves data
with two mechanisms that need neither, and that inject no input and touch no game
memory.

| Direction | Carrier | Receiver reads |
|-----------|---------|----------------|
| addon → companion | a strip of coloured cells in the game window's top-left | pixels from a screen capture |
| companion → addon | load-on-demand slot addons | Lua globals set by files read from disk |

**Outbound** the addon paints the message as 3-bit cells (pure primaries, 4 px
each, ~3.5 KB per frame) anchored so one UI unit is exactly one physical pixel.
The companion finds the strip by its fixed magic bytes and decodes it.

**Inbound** `ocw install` pre-creates 200 `## LoadOnDemand: 1` addons;
`C_AddOns.LoadAddOn` re-reads a slot's Lua from disk when the game loads it. The
companion writes the current state of every chat into all 200, and the addon
loads a fresh one on a timer or when a signal fires. No fonts, no chat log, no
manual calibration.

See [docs/protocol.md](docs/protocol.md) for the byte-level specification and
[docs/architecture.md](docs/architecture.md) for the design.

## Status

| Area | State |
|------|-------|
| Pixel-strip outbound transport | implemented, round-trip tested |
| Load-on-demand slot inbound | implemented, round-trip tested |
| OpenCode V2 session + prompt + polling | implemented, validated live end-to-end |
| Tabs, one OpenCode session each | implemented, isolation verified end-to-end |
| Game-state context (tiered, size-capped) | implemented |
| Git summary in the prompt preamble | implemented |
| `/stop`, `/new`, `/model`, `/help` commands | implemented |
| Live progress, permission prompts | planned |
| Reply echo into a dedicated chat tab, item links | planned |

The transport depends on undocumented client behaviour (the UI-scale trick and
the load-on-demand file rules), validated against WoW Forever
(`wow_classic_beta`, 1.60.1). A client patch can break it.

## Verification

```sh
cd companion
cargo test --offline            # 45 unit + 6 integration tests, all passing
cargo run --offline -- selftest # bridge round-trip, no game client needed
cargo run --offline -- selftest --live --model opencode/mimo-v2.6-flash-free

python3 ../tools/check_lua.py ../addon/OCWow/*.lua
```

| Check | Evidence |
|-------|----------|
| `cargo test` | cell packing, frame encode/decode, Fletcher-16, magic/checksum rejection, strip location at several cell sizes and offsets, PNG decoding, slot-bank generation and publishing, Lua escaping, record parsing, OpenCode reply extraction, context splitting, git formatting |
| `ocw selftest` | strip payload → job → worker → slot bank → reply, in-process, two tabs isolated |
| `ocw selftest --live` | real OpenCode session in a project; "pong" returned through the slot bank; session deleted afterwards |
| `tools/check_lua.py` | Lua block balance, method-call syntax, and use of globals this client omits |

## Quick start

Only the last step needs to run each time you play: `ocw run` **is** the
companion process, so it must be running (in any terminal) while you are in game.

### Two command surfaces

* **Your terminal** runs the `ocw` binary: `install`, `probe`, `run`, `ping`,
  `models`, `projects`, `selftest`, `paths`.
* **WoW's chat box** runs slash commands: `/ocw`, `/ocw tab <n>`, `/ocw newtab`,
  `/ocw closetab`, `/ocw rename`, `/ocw ctx`, `/ocw context on|off`, `/ocw test`,
  `/ocw resend`, `/ocw status`, `/ocw transport on|off`, and `/ai <text>` to send
  from the chat box.

### One-time setup

```sh
cd companion
cargo build --release

# install the addon and create the slot bank (200 addons beside OCOWow)
./target/release/ocw install --addon-dir "/Applications/World of Warcraft/_classic_beta_/Interface/AddOns/OCWow"
```

**Fully quit and relaunch WoW** — the client only discovers files that existed at
launch. Enable *OCWow* on the AddOns screen and leave the `OCWow Slot …` entries
disabled; they are transport files.

Then, in game, run `/ocw test` to put the strip on screen, and in a terminal:

```sh
./target/release/ocw probe
```

`probe` captures the screen, finds the strip, and remembers where it is.

On macOS, `probe` and `run` need **Screen Recording** permission for your
terminal (System Settings → Privacy & Security → Screen Recording), and WoW must
be windowed or borderless — exclusive fullscreen blocks capture.

### Every play session

```sh
./target/release/ocw run --project ~/code/my-project
```

Leave it running while you play. In game: `/ocw`, type a prompt, press Enter.
`ocw run --mock` uses a local echo backend, the fastest way to verify the
transport before involving a model.

## Updating

| Changed | Rebuild the binary? | Re-run install? |
|---------|--------------------|-----------------|
| Addon Lua | no, with `--from` | yes (`--from addon/OCWow --no-slots`) |
| Wire protocol / slots | yes | yes, then restart WoW |

Fast loop while editing the addon — no Rust rebuild:

```sh
ocw install --from addon/OCWow --no-slots
```

Then `/reload` in game. A full client restart is only needed when the slot bank
changes, because the client discovers addon files at startup.

## Commands

| Command | Purpose |
|---------|---------|
| `ocw install` | write the addon files and create the slot bank |
| `ocw probe` | find the pixel strip on screen and remember it |
| `ocw run` | serve the bridge (`--mock` for a local echo backend) |
| `ocw selftest` | in-process bridge round-trip (`--live` to call OpenCode) |
| `ocw ping` / `ocw models` / `ocw projects` | inspect the OpenCode server |
| `ocw paths` | show resolved configuration paths |

`ocw run` flags:

| Flag | Meaning |
|------|---------|
| `--mock` | local echo backend |
| `--project DIR` | working directory the agent runs in |
| `--model provider/model` | default model |
| `--capture-cmd TEMPLATE` | custom capture command |
| `--poll-ms N` | strip sampling interval (default 250 ms) |
| `--duration N` | stop after N seconds |
| `--verbose` | log decoded frames and publishes |

## Game context

The addon attaches a delimited, size-capped snapshot of game state (the
companion truncates it at 1,200 bytes). Three tiers:

* **light** (default) — `player`, `location` (zone/subzone/coords), `group`,
  `in_combat`, `resting`, `instance`.
* **normal** — adds `target`, `focus`, `vitals`, `gear`, `money`, `durability`,
  `bags`, `quests`.
* **full** — adds `guild`, `professions`, `played_seconds`.

Cycle with `/ocw ctx`, or disable with `/ocw context off`. Combat values this
client marks "secret" are dropped rather than formatted.

## OpenCode integration

The companion targets the OpenCode **V2 HTTP API** and discovers the running
service from `~/.local/state/opencode/service.json`.

* Auth is HTTP Basic with the literal username `opencode` and the service
  password.
* Requests are scoped to a project with the `x-opencode-directory` header.
* `POST /api/session/{id}/prompt` **enqueues** and returns immediately, so the
  companion polls `GET /api/session/{id}/message` until the newest assistant
  message has a `finish` value.
* The project directory **must** be in the session-create body; the
  `x-opencode-directory` header is ignored by `POST /api/session`.

## Requirements

- Rust 1.74+ (to build the companion). Builds work offline; five dependencies.
- Python 3 (only for the optional Lua check).
- WoW windowed or borderless, with the game window on screen.
- macOS: Screen Recording permission for your terminal.
- A running OpenCode service (`opencode2`).

## Documentation

- [docs/setup.md](docs/setup.md) — install, updating, troubleshooting.
- [docs/protocol.md](docs/protocol.md) — the wire protocol.
- [docs/architecture.md](docs/architecture.md) — components, data flow, decisions.
- [AGENTS.md](AGENTS.md) — guidance for agents working on the repo.

## License

MIT — see [LICENSE](LICENSE). The transport design follows
[`chelinho139/wow-claude`](https://github.com/chelinho139/wow-claude) and
[`0xinuarashi/wow-forever-codex`](https://github.com/0xinuarashi/wow-forever-codex);
the code here is an independent implementation.
