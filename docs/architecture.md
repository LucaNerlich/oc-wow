# Architecture

## Components

### Addon (`addon/OCWow/`, Lua)

| File | Responsibility |
|------|----------------|
| `Codec.lua` | cell packing, frame encoding, Fletcher-16 |
| `Context.lua` | tiered game-state snapshot, secret-value filtering |
| `UI.lua` | panel: tab bar, per-tab transcript, input, status |
| `Main.lua` | strip drawing, slot loading, signals, tabs, slash commands |

Each **tab** is an independent OpenCode session with its own transcript and at
most one in-flight request. The ticker runs every two seconds: it checks ack
signals, re-shows unacknowledged messages, and loads a slot on schedule or when a
readiness signal fires.

### Companion (`companion/`, Rust)

| Module | Responsibility |
|--------|----------------|
| `protocol/strip.rs` | the pixel-strip wire format |
| `capture/` | screen capture, PNG decoding, strip location |
| `slots.rs` | slot-bank generation, publishing, signal files |
| `http.rs` | minimal blocking HTTP/1.1 client |
| `backend/` | OpenCode V2 client and a local mock |
| `context.rs` | prompt preamble, game-state wrapping and truncation |
| `git.rs` | lightweight `git` summary |
| `app.rs` | main loop, worker thread, job queue, publishing |
| `state.rs` | persisted state |

## Data flow

```
in-game prompt
  └─ Context.snapshot(tier) ──► record: session/tab/request/cwd/flags/name/[ctx/]text
       └─ Codec.EncodeFrame ──► 3-bit cells ──► strip in the window's top-left
            │ screen capture
   capture::find_strip (validate by decoding)
            │ sample_strip → cells → bytes → decode_frame → parse_records
   app::on_prompt ──► Job
            │
   ┌────────┴─────────┐
   │  worker thread   │
   │  context::compose│
   │  OpenCode::ask   │
   │   POST /api/session/{id}/prompt
   │   GET  /api/session/{id}/message
   └────────┬─────────┘
            │ JobStatus (Arc<Mutex<…>>)
   app::publish_now ──► slots::render_slot ──► all 200 Inbox.lua
            │ + raise sig/<slot>.wav
   addon: LoadAddOn("OCWow_Snnn") ──► OCOWow_SlotData
            │ apply replies by (tab, request)
   tab transcript
```

## Key design decisions

**Why the pixel strip for outbound?** An addon cannot open a socket or read a
file at runtime, but anything it draws can be captured by another process. Two
earlier attempts failed for reasons worth remembering: the **chat log** only
records real chat messages, so addon `AddMessage` output never reaches
`WoWChatLog.txt`; and **glyph metrics** are limited by the client's font atlas,
which crashes on hundreds of large glyphs. Pixels have neither problem.

**Why 3-bit pure primaries?** Each channel is fully on or off, giving eight
colours that survive any gamma or contrast setting — intermediate levels do not.
The addon scales its frame so one UI unit is exactly one physical pixel, which
makes the strip a fixed size on screen regardless of UI scale.

**Why validate strip candidates by decoding?** The magic bytes alone match at the
wrong cell size often enough to matter. Requiring the frame to decode (magic,
length, Fletcher-16) removes false positives entirely, and lets `probe` search
cell sizes rather than assuming four.

**Why load-on-demand slots for inbound?** `C_AddOns.LoadAddOn` reads an addon's
Lua from disk when it loads, which is a documented file read in a sandbox that
has none. A slot file is plain Lua, so replies are carried whole with no encoding
and no size limit — unlike the font channel's 476 bytes per packet. The cost is
that each slot is single-use per UI session, hence a pool of 200 and a `/reload`
to free them.

**Why the empty-file signal?** `PlaySoundFile` reports whether a file will play,
and an empty file will not. A pre-created empty `.wav` is therefore a one-shot
flag the companion can raise and the addon can poll for free — far cheaper than
loading a slot just to ask "anything new?".

**Why does the companion write every slot on each publish?** It cannot know which
slot the game will load next. Writing all 200 (atomic rename each) is ~1 MB and
takes milliseconds, and it means whichever slot loads is current.

**Why tabs?** Each tab is an independent OpenCode session, so the companion keeps
a `tab → session id` map and keys jobs by `(tab, request)`. Closing a tab sends a
`d` record so the companion drops the mapping.

**Why a worker thread?** A prompt can take minutes. The read loop must keep
sampling the strip and publishing slots meanwhile, so the backend runs off-thread
and publishes status through a shared `Arc<Mutex<JobStatus>>`.

## Reliability model

* Every frame is checksummed; corrupt frames are dropped.
* Messages are deduplicated by id, so a re-shown strip is harmless.
* An unacknowledged message is re-shown up to three times over 40 s each.
* Signals are best-effort: if the sound channel misbehaves (or the client reports
  an empty file as playable), the addon self-tests at login and falls back to
  scheduled slot polling.
* A stale cached strip rectangle triggers a re-search on the next frame.

## Roadmap

* **Progress and permissions** — surface OpenCode tool events and permission
  requests in the panel, with an Allow & retry button.
* **Chat polish** — echo replies into a dedicated chat tab (`FCF_OpenNewWindow`),
  `/r` to reply to the last speaker, shift-click item/spell/quest links.
* **Per-tab project folders**, so tabs can work in different repositories.

## Threat model / caveats

* The companion runs an agent that can read and write files in the configured
  project. Prefer a read-only posture until you trust the setup.
* Slot files and companion state can contain prompt and reply text on disk.
* The transport is not authenticated: any local process that can write the slot
  files or draw over the strip could forge messages. It is a local convenience
  channel, not a security boundary.
* The transport depends on undocumented client behaviour and a client patch can
  break it.
