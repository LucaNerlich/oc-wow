# AGENTS.md

Guidance for agents working on OCWow.

## What this is

A World of Warcraft addon (Lua) plus a Rust desktop companion that lets a player
talk to a local OpenCode agent from inside the game. Two transports connect them:

* **outbound** — the addon paints a message as a strip of coloured cells in the
  game window's top-left; the companion screen-captures that region and decodes
  it;
* **inbound** — the companion writes the latest state of every chat into a bank
  of load-on-demand slot addons, which the game reads from disk when it loads
  one.

Read `docs/architecture.md` and `docs/protocol.md` before changing anything that
crosses the boundary.

## Layout

```
addon/OCWow/      Lua addon (embedded into the binary at build time)
companion/        Rust crate `ocw` (lib + bin)
  src/protocol/   the pixel-strip wire format
  src/capture/    screen capture, PNG decoding, strip location
  src/slots/      slot-bank generation and publishing (src/slots.rs)
  src/backend/    OpenCode V2 client + mock
  src/app.rs      main loop, worker thread, job queue
tools/            Python dev tools (Lua check)
docs/             protocol, architecture, setup
```

## Build and test

```sh
cd companion
cargo test --offline                 # unit + integration tests
cargo build --offline
cargo run --offline -- selftest      # in-process bridge round-trip
cargo run --offline -- selftest --live --model opencode/mimo-v2.6-flash-free
```

Offline builds are required: the environment has no crates.io access, so only
cached crates are available. The dependency set is deliberately tiny (`anyhow`,
`clap`, `serde`, `serde_json`, `miniz_oxide`). Do not add a dependency without
checking `cargo build --offline` still works.

```sh
python3 tools/check_lua.py addon/OCWow/*.lua      # Lua balance, method calls, stripped globals
```

## Rules of the road

* **The wire format is implemented twice**: `addon/OCWow/Codec.lua` encodes the
  strip and `companion/src/protocol/strip.rs` decodes it. Any change must be made
  in both, with a round-trip test in `companion/tests/roundtrip.rs`, and
  `docs/protocol.md` updated.
* **The strip must stay findable.** Cells are pure primaries at a fixed 4 px;
  the frame starts with fixed magic bytes and ends with a Fletcher-16 checksum.
  `capture::scan::find_strip` validates candidates by decoding them — keep that,
  or a wrong cell size will match by accident.
* **Slots are single-use per UI session.** Never assume a slot can be loaded
  twice; `/reload` is what frees the pool. The companion writes every slot on
  each publish because it cannot know which one the game will load.
* **Signal files must exist at launch.** The client only discovers files that
  existed when it started, so `ocw install` pre-creates them empty and the
  companion fills one in to raise it. A raised file stays raised for the client
  process, so treat "already valid" as unreliable and fall back to polling.
* **No input injection, no game memory access, no process memory reads.** The
  whole design exists to avoid those; a change that introduces them is wrong.
* **The chat log cannot carry addon output.** The client only logs real chat
  messages; `AddMessage`/`print` never reach `WoWChatLog.txt`. Do not reintroduce
  it as a channel.
* Lua is 5.1, and this client omits parts of the standard library: no `goto`, no
  integer division (use `math.floor`), and no `math.randomseed`, `math.random`,
  `unpack`, `loadstring`, `dofile`, `loadfile`, `require`. `tools/check_lua.py`
  fails on those; keep it green.
* **Always guard SavedVariables with `if type(DB) ~= "table" then DB = {} end`.**
  The client does not pre-create the table, and it will persist `DB = nil` if the
  global was nil at logout — which then re-nils it on every subsequent load.
* **Never format or concatenate a value from a combat API without checking it.**
  This client returns "secret" values (`issecretvalue`) from `UnitHealth`,
  `UnitPower` and friends; formatting one yields a secret *string*, and
  `table.concat` rejects it. Route such calls through `Context.call`, which drops
  secrets, and keep the `pcall` wrapper on `C.snapshot`.

## Testing changes

* Wire format: round-trip test in `companion/tests/roundtrip.rs`, plus the
  codec's own tests in `protocol/strip.rs`.
* Strip location: `capture/scan.rs` tests paint synthetic frames at several cell
  sizes and offsets.
* App changes: extend `ocw selftest` (offline) and, for backend changes, the
  `--live` variant.
* Lua changes: `tools/check_lua.py`, then a real `/reload` in game.

## Known empirical dependencies

The scale trick (one UI unit = one physical pixel) and the load-on-demand file
rules are undocumented client behaviour, validated against WoW Forever
(`wow_classic_beta`, 1.60.1). If the strip cannot be found, suspect the capture
(permission, windowed mode, the window being off screen) before the encoding.
