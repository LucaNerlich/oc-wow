# OCWow

Drive [OpenCode](https://opencode.ai) from inside World of Warcraft.

OCWow is a WoW addon plus a small desktop companion. You type a prompt into an
in-game panel, keep playing, and the answer comes back into the game as normal
text. It works for WoW questions ("what's my current quest?", "is this item an
upgrade?") and for real coding work in any project on your machine.

```
  WoW addon (Lua, sandboxed)              Companion (Rust, cross-platform)        OpenCode
  ──────────────────────────              ────────────────────────────────        ────────
  panel: chat | context | commands        pixel-strip decoder  (screen capture) ─┐
   • prompt box                           font-metric encoder  (TTF writer)    ─┘ transport
   • game-state context tiers             OpenCode HTTP client ────────────────► serve (V2 API)
   • /model /new /stop commands           git / VCS reporting
   • completion + error messages          context assembler
```

## How the two channels work

WoW addons are sandboxed: no sockets, no file reads. OCWow moves data with two
mechanisms that need neither, and that inject no input and touch no game memory.

| Direction | Carrier | What the receiver reads |
|-----------|---------|-------------------------|
| addon → companion | a 128×4 black/white pixel strip | bits sampled from a screen capture |
| companion → addon | a generated TrueType font | glyph advance widths via `GetStringWidth` |

The addon paints 64 bytes per frame onto a small strip. The companion
screen-captures just that region and decodes Adler-32-checked packets. For the
return path the companion writes a font whose glyph advances encode bytes
0–255; the addon loads the font and measures text widths to recover the reply
as an ordinary Lua string. Because the client caches a font after its first
load, the installer pre-creates a bank of unused font filenames that the
companion replaces one at a time — so ordinary messages need no `/reload`.

See [docs/protocol.md](docs/protocol.md) for the byte-level specification and
[docs/architecture.md](docs/architecture.md) for the design.

## Status

| Area | State |
|------|-------|
| Pixel-strip outbound transport | implemented, protocol-tested (not yet confirmed in-game) |
| Font-metric inbound transport | implemented, round-trip tested, independently validated |
| OpenCode V2 session + prompt + polling | implemented, validated live end-to-end |
| Game-state context (tiered, size-capped) | implemented |
| Git summary in the prompt preamble | implemented |
| `/stop`, `/new`, `/model`, `/help` commands | implemented |
| In-game project/session/model pickers | planned |
| In-game Git tab | planned |
| Windows / Linux capture templates | documented, not shipped |

The pixel strip and the font-caching behaviour are empirical: they have been
validated for WoW Forever (`wow_classic_beta`, 1.60.1) and depend on
undocumented client behaviour. A client patch can break the transport.

## Verification

Every claim above is backed by a repeatable check. From a clean checkout:

```sh
cd companion
cargo test --offline            # 52 unit + 4 integration tests, all passing
cargo run --offline -- selftest # bridge round-trip, no game client needed
cargo run --offline -- selftest --live --model opencode/mimo-v2.6-flash-free

python3 ../tools/check_lua.py ../addon/OCWow/*.lua
python3 ../tools/validate_font.py target/debug/...   # macOS CoreText check
```

What each covers:

| Check | Evidence |
|-------|----------|
| `cargo test` | frame encode/decode, Adler-32 vectors, pixel strip render/decode/search, TTF structure + checksums + byte round-trip, PNG decoding, HTTP chunked parsing + base64, OpenCode reply extraction, context splitting/truncation, git formatting, fragment reassembly |
| `ocw selftest` | prompt frame → job → worker → font bank → decoded reply, in-process |
| `ocw selftest --live` | real OpenCode session in a project; "pong" returned through the font bank; session deleted afterwards |
| `tools/validate_font.py` | macOS CoreText loads the generated TTF and confirms units-per-em, cmap glyph ids, and that advance widths encode the expected bytes |
| `tools/check_lua.py` | Lua block balance (no interpreter required) |

The pixel strip is the one piece that cannot be tested without a display; it is
covered by protocol tests plus `ocw probe` and startup auto-calibration, and
needs one confirmation in game.

## Quick start

```sh
cd companion
cargo build --release          # produces target/release/ocw

# 1. install the addon and build the font bank
./target/release/ocw install --addon-dir "/Applications/World of Warcraft/_classic_beta_/Interface/AddOns/OCWow"

# 2. enable OCWow in the addon list, then /reload; run /ocw calibrate in game

# 3. find the strip on screen (optional: run auto-calibrates at startup)
./target/release/ocw probe

# 4. serve the bridge
./target/release/ocw run --project ~/code/my-project
```

Then in game: `/ocw`, type a prompt, press Enter.

`ocw run --mock` uses a local echo backend, which is the fastest way to verify
the transport before involving a model.

If the crop is wrong, `run` captures once at startup, searches for the strip,
and adopts what it finds — so `probe` is mainly a diagnostic and a way to
persist an exact crop.

## Commands

| Command | Purpose |
|---------|---------|
| `ocw install` | write addon files and create the font bank |
| `ocw probe` | capture once and locate the pixel strip (calibration) |
| `ocw run` | serve the bridge (`--mock` for a local echo backend) |
| `ocw selftest` | in-process bridge round-trip (`--live` to call OpenCode) |
| `ocw ping` / `ocw models` / `ocw projects` | inspect the OpenCode server |
| `ocw dump --slot N` | decode the reply stored in a font slot |
| `ocw font --out F --text "..."` | build a reply font from text (debugging) |
| `ocw paths` | show resolved configuration paths |

`ocw run` flags:

| Flag | Meaning |
|------|---------|
| `--mock` | local echo backend |
| `--project DIR` | working directory the agent runs in; new sessions are scoped to it |
| `--model provider/model` | default model |
| `--crop x,y,w,h` \| `full` | strip location |
| `--cell N` | cell size in pixels (`0` = auto, required for Retina) |
| `--capture-cmd TEMPLATE` | custom capture command |
| `--poll-ms N` | strip sample interval (default 120 ms) |
| `--duration N` | stop after N seconds |
| `--verbose` | log decoded frames and slot writes |

In-game:

| Input | Purpose |
|-------|---------|
| `/ocw` | toggle the panel |
| `/ocw calibrate` | show a fixed strip frame for calibration |
| `/ocw ctx` | cycle game context: light → normal → full |
| `/ocw context on\|off` | include or omit game state |
| `/ocw new`, `/ocw stop`, `/ocw model <provider/model>`, `/ocw help` | session commands |
| `/ocw status` | transport diagnostics |

## Game context

The addon attaches a delimited, size-capped snapshot of game state (the
companion truncates it at 1,200 bytes). Three tiers:

* **light** (default) — `player`, `location` (zone/subzone/coords), `group`,
  `in_combat`, `resting`, `instance`.
* **normal** — adds `target`, `focus`, `vitals`, `gear` (avg item level),
  `money`, `durability`, `bags`, `quests`.
* **full** — adds `guild`, `professions`, `played_seconds`.

Cycle with `/ocw ctx`, or disable entirely with `/ocw context off`. The
companion frames the block so the model treats it as situational metadata, and
prepends a short preamble plus a git summary (branch, ahead/behind, changed
files, last commits) for the active project.

## OpenCode integration

The companion targets the OpenCode **V2 HTTP API** and discovers the running
service (URL and password) from `~/.local/state/opencode/service.json`, so it
follows whatever port your background service bound to.

Notes for anyone extending this:

* Auth is HTTP Basic with the literal username `opencode` and the service
  password.
* Requests are scoped to a project with the `x-opencode-directory` header.
* `POST /api/session/{id}/prompt` **enqueues** and returns immediately, so the
  companion polls `GET /api/session/{id}/message` until the newest assistant
  message has a `finish` value.
* Sessions are created with `POST /api/session` (`{title, model:{id,providerID},
  location:{directory}}`); models come from `GET /api/model`; interrupt is
  `POST /api/session/{id}/interrupt`.
* The project directory **must** be sent in the session-create body. The
  `x-opencode-directory` header scopes GETs such as `/api/model` and
  `/api/vcs`, but is ignored by `POST /api/session`; relying on the header alone
  silently creates the session in the server's working directory.

## Requirements

- Rust 1.74+ (to build the companion). Builds work offline with the vendored
  dependency cache; the dependency set is deliberately tiny.
- Python 3 (only for the optional dev tools).
- WoW running windowed or borderless with the strip visible and unobscured.
- macOS: Screen Recording permission for your terminal.
- A running OpenCode service (`opencode2`).

## Documentation

- [docs/setup.md](docs/setup.md) — install, calibration, cross-platform capture
  templates, troubleshooting.
- [docs/protocol.md](docs/protocol.md) — the byte-level wire protocol.
- [docs/architecture.md](docs/architecture.md) — components, data flow, design
  decisions, roadmap.
- [AGENTS.md](AGENTS.md) — guidance for agents working on the repo.

## Development

```sh
cd companion && cargo test --offline
cargo run --offline -- selftest
cargo run --offline -- selftest --live --model opencode/mimo-v2.6-flash-free
python3 tools/check_lua.py addon/OCWow/*.lua
python3 tools/validate_font.py <generated font>
```

## License

MIT — see [LICENSE](LICENSE). The transport technique is inspired by
[`0xinuarashi/wow-forever-codex`](https://github.com/0xinuarashi/wow-forever-codex);
the protocol and all code here are an independent implementation.
