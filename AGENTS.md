# AGENTS.md

Guidance for agents working on OCWow.

## What this is

A World of Warcraft addon (Lua) plus a Rust desktop companion that lets a
player talk to a local OpenCode agent from inside the game. Two custom
transports connect them: a black/white pixel strip (addon → companion) and
generated font metrics (companion → addon).

Read `docs/architecture.md` and `docs/protocol.md` before changing anything
that crosses the boundary. **The protocol is implemented twice** — once in
`addon/OCWow/Protocol.lua` and `addon/OCWow/Native.lua`, and once in
`companion/src/protocol/` and `companion/src/fonts/`. Any wire change must be
made in both places, and `protocol::frames::PROTOCOL_VERSION` plus the version
byte in the Lua encoders must be bumped together.

## Layout

```
addon/OCWow/      Lua addon (embedded into the binary at build time)
companion/        Rust crate `ocw` (lib + bin)
  src/protocol/   frames, adler32, pixel strip
  src/fonts/      TrueType writer + font bank
  src/capture/    PNG decoder + screen capture
  src/backend/    OpenCode V2 client + mock
  src/app.rs      main loop, worker thread, job queue
tools/            Python dev tools (Lua check, font validation)
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
cached crates are available. The current dependency set is deliberately tiny
(`anyhow`, `clap`, `serde`, `serde_json`, `miniz_oxide`, `base64`). Do not add
a dependency without checking `cargo build --offline` still works.

```sh
python3 tools/check_lua.py addon/OCWow/*.lua      # Lua block balance
python3 tools/validate_font.py <font.ttf>         # macOS CoreText validation
```

## Rules of the road

* **Never rewrite a font slot the addon may have already loaded.** Slots are
  consumed monotonically; reuse only after the companion detects a new WoW
  process id.
* **Keep both protocol implementations in sync** and update `docs/protocol.md`.
* **Keep the context block capped** (`context::MAX_CONTEXT_BYTES`) so game
  state cannot dominate a prompt.
* **No input injection, no game memory access, no process memory reads.** The
  whole design exists to avoid those; a change that introduces them is wrong.
* Lua is 5.1: no `goto`, no integer division, use `math.floor`. Guard WoW API
  calls that differ across client builds with `pcall`.
* The addon must never assume a file exists; it only measures fonts by path.

## Testing changes

* Wire changes: add a round-trip test in `companion/tests/roundtrip.rs`.
* Font changes: `cargo test` covers the round trip; `tools/validate_font.py`
  independently validates structure and advances.
* App changes: extend `ocw selftest` (offline) and, for backend changes, the
  `--live` variant.
* Lua changes: `tools/check_lua.py`, then a real `/reload` in game.

## Known empirical dependencies

The pixel strip and the font-caching behaviour are undocumented client
behaviour, validated against WoW Forever (`wow_classic_beta`, 1.60.1). If the
strip decodes but replies never arrive, suspect the font-caching assumption
first: check `ocw dump --slot N` and the `slot` mismatch path in
`Native.parse_packet`.
