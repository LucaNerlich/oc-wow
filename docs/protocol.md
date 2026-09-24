# OCWow transport protocol

Version 2. Two channels, both using documented addon APIs and OS-level reads.
No input injection, no game memory access.

| Direction | Carrier | Receiver reads |
|-----------|---------|----------------|
| addon → companion | a strip of coloured cells in the game window's top-left | pixels from a screen capture |
| companion → addon | load-on-demand slot addons | Lua globals set by files read from disk |

Earlier revisions used the chat log and generated fonts. Both are gone: the
client does not write addon frame messages to `WoWChatLog.txt`, and glyph
metrics are limited by the client's font atlas.

## 1. Outbound: the pixel strip

The addon draws a message as a grid of cells anchored at `UIParent`'s top-left,
with the frame scaled by `768 / physicalScreenHeight` so that **one UI unit is
exactly one physical pixel**. The strip is therefore a fixed size on screen
whatever the player's UI scale.

* **Cells are 3 bits**, one per colour channel, each fully on or off — eight pure
  primaries, which survive any gamma or contrast setting.
* **4×4 pixels per cell**, 200 cells per row, up to 48 rows.
* Cells are packed 3 bits at a time, **MSB-first**, into a byte stream.
* The byte stream is `[magic][id][len][payload…][Fletcher-16]`:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | magic `0xC7 0x1A` |
| 2 | 2 | message id (big-endian) |
| 4 | 2 | payload length (big-endian) |
| 6 | len | payload |
| 6+len | 2 | Fletcher-16 over bytes `2..6+len`, each sum mod 255 |

Capacity is ~3.5 KB per frame. The strip stays up until the companion
acknowledges the message (see signals) or 40 s pass, then it is re-shown up to
three times.

### Records

The payload is one or more records separated by `\x1E`, fields by `\x1F`:

```
session \x1F tab \x1F request \x1F cwd \x1F flags \x1F name \x1F [context \x1F] text
```

* `session` — random per SavedVariables lifetime; the companion dedups on
  `(session, id)`.
* `tab` — which UI tab (i.e. which OpenCode session) the message belongs to.
* `request` — per-tab counter.
* `flags` — `h` hello (announce the session, no prompt), `n` start a fresh
  OpenCode session, `d` the tab was closed (forget its session), `c` a context
  field follows `name`.
* `context` — only present with the `c` flag, so a separator inside the text
  cannot be mistaken for one. A few lines about the character and location,
  capped by the addon.
* `text` — the message.

### Finding the strip

Every frame starts with the same six cell colours, which is what `ocw probe`
searches a full-screen capture for. A candidate is only accepted if the frame
**decodes**: that check is what prevents a wrong cell size from producing a
false match. The found rectangle is cached in the config; later captures only
grab that region.

## 2. Inbound: load-on-demand slot addons

WoW addons cannot read files at runtime, but `C_AddOns.LoadAddOn` reads an
addon's Lua from disk at the moment it loads. `ocw install` therefore creates
`OCWow_S001` … `OCWow_S200`, each `## LoadOnDemand: 1` with a single
`Inbox.lua`. Each slot can be loaded once per UI session; `/reload` frees them
all.

The companion does not know which slot the game will load next, so every publish
writes the same content to all 200 (atomic rename per file). The content is the
latest status of every chat:

```lua
OCWow_SlotData = {
  now = <companion epoch seconds>,
  seq = <publish counter>,
  replies = {
    { tab = 1, request = 3, status = "working"|"done"|"error",
      text = "...", session = "ses_…", denied = { "Bash(rm:*)" } },
  },
}
```

The addon loads a fresh slot on a schedule after each send (5, 10, 16, 24, 34,
46, 60, 80, 100, 130, 160, 200, 240, 300 s, then every 60 s) or immediately when
a readiness signal fires, and matches replies by `(tab, request)`. `now` lets the
addon know how long ago the companion last wrote (same machine, so the clocks
agree).

Slot files have no size limit, so a reply is carried whole — unlike the font
channel, which capped a message at 476 bytes per slot.

## 3. Signals: the empty-file trick

`PlaySoundFile(path)` reports whether a file will play. An empty file will not; a
valid one will; and a file that has never been loaded is read fresh. So a
pre-made empty `.wav` is a one-shot flag the companion can raise at any time and
the addon can poll for free.

| File | Raised when |
|------|-------------|
| `sig/NNN.wav` | reply NNN is ready → load a slot now instead of waiting |
| `ack/NNN.wav` | the companion received message NNN → take it off the strip |
| `ctl/empty.wav`, `ctl/valid.wav` | never change; the addon checks at login that empty reads as unplayable and valid as playable, and disables the channel if not |

`NNN = ((id − 1) mod 200) + 1`. A raised file stays playable for the rest of the
client process, so an unexpected "already valid" is treated as unreliable and the
addon falls back to scheduled polling. With the sound channel off everything
still works, just with coarser timing.

## 4. States

| Value | Name | Meaning |
|-------|------|---------|
| — | working | the model is generating (or the job is queued) |
| — | done | complete reply |
| — | error | the run failed; the text carries the message |
