--[[
OCWow :: Protocol

Wire protocol shared with the desktop companion.

Outbound (addon -> companion) travels as a 128x4 black/white pixel strip:
  * 512 cells, MSB-first, 8 cells per byte => 64 bytes per displayed frame.
  * Prompt frames carry UTF-8 prompt fragments (up to 40 bytes each).
  * Control frames advertise the font slot the addon is about to load and the
    reply fragment it needs.

Inbound (companion -> addon) travels as glyph advance widths inside a generated
TrueType font; see Native.lua.

All multi-byte integers are big-endian and every frame ends with an Adler-32
checksum. See docs/protocol.md.
]]

local _, ns = ...
local OCWow = ns or OCWow

local P = {}
OCWow.Protocol = P

P.STRIP_COLS = 128
P.STRIP_ROWS = 4
P.CELL_PX = 4
P.CELL_COUNT = P.STRIP_COLS * P.STRIP_ROWS
P.BYTES_PER_FRAME = P.CELL_COUNT / 8

P.OUT_MAGIC = { 0x4F, 0x43 } -- "OC"
P.IN_MAGIC = { 0x43, 0x46 } -- "CF"
P.VERSION = 1

P.TYPE_PROMPT = 1
P.TYPE_CONTROL = 2

P.PROMPT_PAYLOAD_MAX = 40
P.REPLY_PAYLOAD_MAX = 476
P.MAX_PROMPT_BYTES = P.PROMPT_PAYLOAD_MAX * 32

-- Delimiters around the game-state block; must match the companion's context.rs.
P.CONTEXT_OPEN = "<<<WOW_STATE"
P.CONTEXT_CLOSE = "WOW_STATE>>>"

-- Reply / control states.
P.STATE = {
	WAITING = 0,
	QUEUED = 1,
	WORKING = 2,
	STREAMING = 3,
	DONE = 4,
	FAILED = 5,
	INTERRUPTED = 6,
	RESET = 7,
}

P.STATE_NAME = {
	[0] = "waiting",
	[1] = "queued",
	[2] = "working",
	[3] = "streaming",
	[4] = "done",
	[5] = "failed",
	[6] = "interrupted",
	[7] = "reset",
}

local MOD_ADLER = 65521

--- Adler-32 over the first `len` bytes of `bytes` (1-based array).
function P.adler32(bytes, len)
	local a, b = 1, 0
	len = len or #bytes
	for i = 1, len do
		a = (a + (bytes[i] or 0)) % MOD_ADLER
		b = (b + a) % MOD_ADLER
	end
	return b * 65536 + a
end

--- UTF-8 encode a code point (BMP only).
function P.utf8char(cp)
	if cp < 0x80 then
		return string.char(cp)
	elseif cp < 0x800 then
		return string.char(0xC0 + math.floor(cp / 0x40), 0x80 + (cp % 0x40))
	else
		return string.char(
			0xE0 + math.floor(cp / 0x1000),
			0x80 + (math.floor(cp / 0x40) % 0x40),
			0x80 + (cp % 0x40)
		)
	end
end

--- Convert a string into a 1-based array of byte values.
function P.to_bytes(s)
	local t = {}
	for i = 1, #s do
		t[i] = string.byte(s, i)
	end
	return t
end

local function put16(t, pos, v)
	t[pos + 1] = math.floor(v / 256) % 256
	t[pos + 2] = v % 256
end

local function put32(t, pos, v)
	t[pos + 1] = math.floor(v / 16777216) % 256
	t[pos + 2] = math.floor(v / 65536) % 256
	t[pos + 3] = math.floor(v / 256) % 256
	t[pos + 4] = v % 256
end

--- Build a 64-byte prompt frame.
function P.encode_prompt(ui_session, request_id, frag_index, frag_count, payload)
	local t = {}
	t[1], t[2] = P.OUT_MAGIC[1], P.OUT_MAGIC[2]
	t[3] = P.TYPE_PROMPT
	t[4] = P.VERSION
	put16(t, 4, ui_session)
	put32(t, 6, request_id)
	put16(t, 10, frag_index)
	put16(t, 12, frag_count)
	t[15] = math.min(#payload, P.PROMPT_PAYLOAD_MAX)
	t[16] = 0
	for i = 1, t[15] do
		t[20 + i] = payload[i]
	end
	for i = 21, 60 do
		t[i] = t[i] or 0
	end
	put32(t, 60, P.adler32(t, 60))
	return t
end

--- Build a 64-byte control frame.
function P.encode_control(ui_session, request_id, requested_fragment, slot, deadline_ms, state, active, last_attempted)
	local t = {}
	t[1], t[2] = P.OUT_MAGIC[1], P.OUT_MAGIC[2]
	t[3] = P.TYPE_CONTROL
	t[4] = P.VERSION
	put16(t, 4, ui_session)
	put32(t, 6, request_id)
	for i = 11, 20 do
		t[i] = 0
	end
	put16(t, 20, requested_fragment)
	put16(t, 22, slot)
	put32(t, 24, deadline_ms)
	t[29] = state
	t[30] = active and 1 or 0
	put16(t, 30, last_attempted)
	for i = 31, 60 do
		t[i] = t[i] or 0
	end
	put32(t, 60, P.adler32(t, 60))
	return t
end

--- Split a string into prompt fragments of at most PROMPT_PAYLOAD_MAX bytes.
function P.fragment_prompt(text)
	local bytes = P.to_bytes(text)
	local fragments = {}
	local i = 1
	while i <= #bytes do
		local chunk = {}
		for j = i, math.min(i + P.PROMPT_PAYLOAD_MAX - 1, #bytes) do
			chunk[#chunk + 1] = bytes[j]
		end
		fragments[#fragments + 1] = chunk
		i = i + P.PROMPT_PAYLOAD_MAX
	end
	if #fragments == 0 then
		fragments[1] = {}
	end
	return fragments
end

--- Decode 64 frame bytes into 512 cell booleans (row-major).
function P.cells_from_bytes(bytes)
	local cells = {}
	for i = 0, P.CELL_COUNT - 1 do
		local byte = bytes[math.floor(i / 8) + 1] or 0
		local bit = math.floor(byte / (2 ^ (7 - (i % 8)))) % 2
		cells[i + 1] = bit == 1
	end
	return cells
end

-- ---------------------------------------------------------------------------
-- The on-screen strip
-- ---------------------------------------------------------------------------

--- Create the strip frame with one texture per cell.
function P.create_strip(parent)
	local strip = CreateFrame("Frame", "OCWowStrip", parent)
	strip:SetSize(P.STRIP_COLS * P.CELL_PX, P.STRIP_ROWS * P.CELL_PX)
	strip:SetPoint("TOPLEFT", UIParent, "TOPLEFT", 8, -8)
	strip:SetFrameStrata("BACKGROUND")
	strip.cells = {}
	strip.state = {}
	for row = 0, P.STRIP_ROWS - 1 do
		for col = 0, P.STRIP_COLS - 1 do
			local tex = strip:CreateTexture(nil, "BACKGROUND")
			tex:SetSize(P.CELL_PX, P.CELL_PX)
			tex:SetPoint("TOPLEFT", strip, "TOPLEFT", col * P.CELL_PX, -row * P.CELL_PX)
			tex:SetColorTexture(0, 0, 0, 1)
			strip.cells[row * P.STRIP_COLS + col + 1] = tex
		end
	end
	return strip
end

--- Paint a frame; only cells whose value changed are touched.
function P.render_strip(strip, bytes)
	local cells = P.cells_from_bytes(bytes)
	for i = 1, #cells do
		local bit = cells[i]
		if strip.state[i] ~= bit then
			strip.state[i] = bit
			if bit then
				strip.cells[i]:SetColorTexture(1, 1, 1, 1)
			else
				strip.cells[i]:SetColorTexture(0, 0, 0, 1)
			end
		end
	end
end

--- A stable, valid frame used while idle so calibration can locate the strip.
function P.idle_frame(ui_session)
	return P.encode_control(ui_session or 0, 0, 0, 0, 0, P.STATE.WAITING, 0, 0)
end

return P
