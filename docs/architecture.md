# Architecture

## Components

### Addon (`addon/OCWow/`, Lua)

| File | Responsibility |
|------|----------------|
| `Protocol.lua` | Adler-32, UTF-8 helpers, frame encoding, strip rendering |
| `Native.lua` | font measurement, packet decoding, staged async reads |
| `Context.lua` | tiered game-state snapshot |
| `UI.lua` | panel: transcript, input, buttons, status |
| `Main.lua` | request lifecycle, ticker, slash commands, SavedVariables |

The addon is a state machine driven by a single `OnUpdate` ticker:

* **transmit** — paint prompt fragments, a few passes for redundancy.
* **receive** — advertise the next font slot, wait out the advertised window,
  load and measure the slot, then act on the decoded packet.
* **idle** — paint a fixed valid frame so calibration can find the strip.

### Companion (`companion/`, Rust)

| Module | Responsibility |
|--------|----------------|
| `protocol/` | frames, Adler-32, cell/bit mapping, strip search |
| `fonts/` | hand-rolled TrueType writer and the font bank |
| `capture/` | PNG decoding and screen capture |
| `http.rs` | minimal blocking HTTP/1.1 client (chunked-aware) |
| `backend/` | OpenCode V2 client and a local mock |
| `context.rs` | prompt preamble, game-state wrapping and truncation |
| `git.rs` | lightweight `git` summary |
| `app.rs` | main loop, worker thread, job queue, reply serving |
| `state.rs` | persisted state and WoW process detection |

## Data flow

```
in-game prompt
  └─ Context.snapshot(tier) ──► prompt + <<<WOW_STATE block
       └─ Protocol.fragment_prompt ──► pixel frames ──► strip
                                                          │ screen capture
                                    ┌─────────────────────┘
                        protocol::pixel::decode_strip
                                    │  PromptFrame
                        app::on_prompt  (reassemble, dedupe)
                                    │  Job
                    ┌───────────────┴────────────────┐
                    │        worker thread           │
                    │  context::compose              │
                    │  OpenCode::ask                 │
                    │    POST /api/session/{id}/prompt│
                    │    GET  /api/session/{id}/message│
                    └───────────────┬────────────────┘
                                    │ JobStatus (Arc<Mutex<…>>)
                        app::on_control  (ControlFrame)
                                    │  ReplyFrame
                        fonts::Bank::publish_reply
                                    │  generated TTF into slot S
                    ┌───────────────┴────────────────┐
                    │  addon: SetFont(S), measure     │
                    │  Native.parse_packet            │
                    └───────────────┬────────────────┘
                                    │
                       UI transcript (assembled fragments)
```

## Key design decisions

**Why pixels out and fonts in?** Both are documented-API mechanisms that need
neither input injection nor memory access. A chat-log outbound channel would be
simpler but spams the chat frame and needs `/chatlog`; SavedVariables need a
`/reload` per message. The pixel strip is silent, and font metrics give real
Lua strings back without a reload.

**Why a font bank rather than one font?** The client caches a font after first
load, and filenames created after the UI loads are not discovered. Pre-creating
many names before the game starts sidesteps both.

**Why does the addon own the slot counter?** The addon must know a slot number
before it loads it, and the only inbound channel is the font itself. Persisting
the counter in SavedVariables keeps it monotonic across `/reload`; the companion
detects a client restart by process id and sends a `reset` packet.

**Why hand-rolled HTTP and PNG and TTF?** The companion must build offline and
on macOS, Windows and Linux with a small dependency surface. All three formats
are small enough to implement directly, and it keeps the binary a single static
artifact.

**Why is the context tiered and capped?** Game state is useful but unbounded. A
light tier always fits on one line; richer tiers are opt-in, and the companion
truncates the block at 1,200 bytes so it can never dominate the context window.

**Why a worker thread?** A prompt can take minutes. The capture loop must keep
serving status and control frames meanwhile, so the backend runs off-thread and
publishes status through a shared `Arc<Mutex<JobStatus>>`.

## Reliability model

* Frames are checksummed; corrupt frames are dropped.
* Prompts are transmitted redundantly and re-sent while receiving.
* Prompts are deduplicated by `(ui_session, request_id)`.
* Stale fonts are detected by the `slot` field and retried on a fresh slot.
* Non-terminal replies back off exponentially to conserve font slots.
* A detected client restart rewinds the slot counter.

## Roadmap

* **Phase 2** — in-game pickers for project, session and model; session history.
* **Phase 3** — an in-game Git tab backed by `GET /api/vcs` and
  `GET /api/vcs/status`; a live streaming preview of partial replies.
* **Phase 4** — first-class Windows and Linux capture backends, packaging, and
  a completion badge with sound.

## Threat model / caveats

* The companion runs an agent that can read and write files in the configured
  project. Prefer a read-only posture until you trust the setup.
* Font files and companion state can contain prompt and reply text on disk.
* The transport is not authenticated: any local process that can write the
  strip region or the bank could forge packets. It is a local convenience
  channel, not a security boundary.
* The pixel and font behaviours are empirical and client-version specific.
