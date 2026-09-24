# OCWow transport protocol

Version 1. All multi-byte integers are big-endian. Every frame ends with an
Adler-32 checksum (RFC 1950) over the preceding bytes.

There are two physical channels:

* **Pixels** carry data from the addon to the companion.
* **Font metrics** carry data from the companion to the addon.

## 1. The pixel strip

The addon renders a grid of `STRIP_COLS = 128` columns by `STRIP_ROWS = 4` rows
of square cells. Cell size defaults to 4 screen pixels, so the strip occupies
512×16 screen pixels.

* Cell index is row-major: `index = row * 128 + col`.
* A white cell is a `1` bit; a black cell is a `0` bit.
* Bits are packed MSB-first into bytes: bit `i` lives in byte `i / 8` at
  position `7 - (i % 8)`.
* 512 cells therefore carry **64 bytes** per displayed frame.

The companion samples the centre of each cell (averaging a 3×3 window),
classifies luminance as black (≤ 96), white (≥ 160), or ambiguous — an
ambiguous cell rejects the whole frame, so a misaligned or occluded capture
fails loudly rather than decoding garbage.

The cell size is auto-detected as `captured_width / 128`, which makes Retina and
scaled displays work without extra configuration.

## 2. Outbound frames (addon → companion), 64 bytes

Common header, 20 bytes:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | magic `0x4F 0x43` (`"OC"`) |
| 2 | 1 | type (`1` = prompt, `2` = control) |
| 3 | 1 | protocol version (`1`) |
| 4 | 2 | `ui_session` — random per UI load |
| 6 | 4 | `request_id` |
| 10 | 2 | `fragment_index` |
| 12 | 2 | `fragment_count` |
| 14 | 1 | `payload_len` |
| 15 | 1 | flags |
| 16 | 4 | reserved |
| 20 | 40 | payload (zero-padded) |
| 60 | 4 | Adler-32 over bytes `0..60` |

### 2.1 Prompt frame (`type = 1`)

* `fragment_index` / `fragment_count` describe the split of the prompt text.
* `payload` holds up to 40 UTF-8 bytes.
* At most 32 fragments are used, giving a 1,280-byte prompt limit.

### 2.2 Control frame (`type = 2`)

The header's `fragment_index`/`fragment_count` are unused; fields live in the
payload area:

| Offset | Size | Field |
|--------|------|-------|
| 20 | 2 | `requested_fragment` (1-based) |
| 22 | 2 | `slot` — font-bank slot the addon will load next |
| 24 | 4 | `deadline_ms` — milliseconds until that load |
| 28 | 1 | receiver state (see §4) |
| 29 | 1 | `active` (`1` while watching for a reply) |
| 30 | 2 | `last_attempted_slot` |

Conceptually the addon says: *"for request R, prepare fragment F in slot S
before the deadline"*.

## 3. Inbound frames (companion → addon), 512 bytes

The companion encodes these 512 bytes as the advance widths of 512 data glyphs
in a generated font. The addon recovers them by measurement (§5).

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | magic `0x43 0x46` (`"CF"`) |
| 2 | 1 | protocol version (`1`) |
| 3 | 1 | state (see §4) |
| 4 | 2 | `ui_session` |
| 6 | 4 | `request_id` |
| 10 | 2 | `fragment_index` (1-based) |
| 12 | 2 | `fragment_count` |
| 14 | 2 | `payload_len` |
| 16 | 4 | `revision` |
| 20 | 2 | `slot` |
| 22 | 1 | flags |
| 23 | 9 | reserved |
| 32 | 476 | payload (UTF-8) |
| 508 | 4 | Adler-32 over bytes `0..508` |

`revision` is a checksum of the complete reply plus its state. If it changes
while the addon is assembling fragments, the partial assembly is discarded and
assembly restarts — so a streaming reply never mixes revisions.

`slot` lets the receiver detect a stale font: if a decoded packet's `slot` does
not match the slot that was requested, the client served a previously cached
font and the addon advances to a new slot.

## 4. States

| Value | Name | Meaning |
|-------|------|---------|
| 0 | waiting | no job for this request yet |
| 1 | queued | accepted, not started |
| 2 | working | the model is generating |
| 3 | streaming | partial content available |
| 4 | done | complete reply |
| 5 | failed | error; payload carries the message |
| 6 | interrupted | cancelled |
| 7 | reset | instruction to rewind the addon's slot counter |

Only `done`, `failed` and `reset` are terminal for the addon's receive loop.

## 5. The font-metric channel

The companion writes a complete TrueType font (units per em 1024) into a bank
slot. Glyph assignment:

| Codepoint | Glyph | Advance width | Purpose |
|-----------|-------|---------------|---------|
| `U+007E` (`~`) | 1 | 512 | trailing glyph, cancels out of measurements |
| `U+E000+i` | 2+i | `(16 + byte_i) * 16` | reply byte `i`, for `i` in `0..512` |
| `U+E200` | 514 | 256 | calibration for byte 0 |
| `U+E201` | 515 | 4336 | calibration for byte 255 |

**Glyphs carry no outline.** The data lives only in the `hmtx` advance widths.
This is not a stylistic choice: the client rasterises glyphs into a shared font
atlas while measuring text, and 512 wide glyphs overflow that atlas, crashing
the client with `ASSERTNN(freedPixels >= pixelsNeeded)` in
`GxuFontMiscClasses.cpp`. Empty glyphs cost the atlas nothing and still report
their advance width. `companion/tests` pins this invariant.

The addon measures (font size 64) and normalises:

```text
low  = width(U+E200 .. "~")
high = width(U+E201 .. "~")
read = width(U+E000+i .. "~")
byte = round((read - low) * 255 / (high - low))
```

The trailing glyph cancels in the subtraction, and the calibration glyphs
cancel the UI scale and font size. Nominal widths are `low = 16`, `high = 271`.

## 6. The font bank

Because the client caches a font after its first load in a given process, the
companion never rewrites a slot the addon may have already loaded. Instead the
installer pre-creates `fontreply0001.ttf` … `fontreplyNNNN.ttf` (default 4096),
hard-linked to one baseline font where the filesystem allows, and the companion
publishes into each slot at most once per client process.

Slots are consumed monotonically. The addon remembers its next slot in
SavedVariables so a `/reload` does not rewind it (a rewind would read cached
fonts). When the companion observes a **new WoW process id**, it answers the
next request with a `reset` packet, which rewinds the addon's counter — safe
because a fresh process has no cached fonts.

When the bank is exhausted the addon reports it; restarting the game client and
the companion restarts the bank.

## 7. Reliability

* Every frame is checksummed; bad frames are dropped.
* Prompt fragments are transmitted several times, and one fragment is re-sent
  periodically while receiving, so a missed capture still starts the job.
* The companion deduplicates prompts by `(ui_session, request_id)`.
* A receive attempt that finds a stale or invalid slot advances to a new slot
  and retries; ten consecutive failures abort with a visible in-game message.
* Non-terminal states back off exponentially (up to ~5 s) to conserve slots.
