--[[
OCWow :: Native

Reads reply bytes out of generated fonts.

The companion writes a TrueType font whose glyph advance widths encode reply
bytes. Lua never reads the file; it asks the client to load the font, then
measures text with GetStringWidth. Calibration glyphs normalise the
measurement so UI scale and font size cancel out:

    low  = width(cal_0   .. "~")
    high = width(cal_255 .. "~")
    read = width(data_i  .. "~")
    byte = round((read - low) * 255 / (high - low))

Measurement is asynchronous: a freshly referenced font may not be ready on the
frame it is requested, so reads are staged across frames and validated with the
packet checksum and slot number.
]]

local _, ns = ...
local OCWow = ns or OCWow
local P = OCWow.Protocol

local N = {}
OCWow.Native = N

N.CODEPOINT_DATA_BASE = 0xE000
N.CODEPOINT_CAL_LOW = 0xE200
N.CODEPOINT_CAL_HIGH = 0xE201
N.CODEPOINT_TRAIL = 0x7E

N.DATA_GLYPHS = 512
N.PACKET_BYTES = 512
N.HEADER_BYTES = 32
N.PAYLOAD_MAX = P.REPLY_PAYLOAD_MAX
N.FONT_SIZE = 64
N.FONT_PATH = "Interface\\AddOns\\OCWow\\Fonts\\fontreply%04d.ttf"

N.TRAIL = P.utf8char(N.CODEPOINT_TRAIL)
N.CAL_LOW = P.utf8char(N.CODEPOINT_CAL_LOW)
N.CAL_HIGH = P.utf8char(N.CODEPOINT_CAL_HIGH)

--- Details of the last failed calibration, surfaced in the in-game message.
N.last_calibration = nil

local GLYPHS_PER_FRAME = 48
local MAX_CAL_TRIES = 30
local MIN_CAL_SPAN = 50

local measurer
local worker
local queue = {}
local active

local function data_char(index)
	return P.utf8char(N.CODEPOINT_DATA_BASE + (index - 1))
end

local function decode_byte(low, high, read)
	if high <= low then
		return 0
	end
	local value = math.floor(((read - low) * 255 / (high - low)) + 0.5)
	if value < 0 then
		value = 0
	elseif value > 255 then
		value = 255
	end
	return value
end

local function get16(bytes, pos)
	return bytes[pos + 1] * 256 + bytes[pos + 2]
end

local function get32(bytes, pos)
	return bytes[pos + 1] * 16777216 + bytes[pos + 2] * 65536 + bytes[pos + 3] * 256 + bytes[pos + 4]
end

--- Validate and parse a 512-byte reply packet.
function N.parse_packet(bytes)
	if bytes[1] ~= P.IN_MAGIC[1] or bytes[2] ~= P.IN_MAGIC[2] then
		return nil, "magic"
	end
	if bytes[3] ~= P.VERSION then
		return nil, "version"
	end
	local expected = get32(bytes, 508)
	if expected ~= P.adler32(bytes, 508) then
		return nil, "checksum"
	end

	local payload_len = math.min(get16(bytes, 14), N.PAYLOAD_MAX)
	-- Build the payload string one byte at a time: `unpack` is not guaranteed to
	-- exist in every client build, and `string.char` with ~500 arguments is a
	-- needless risk.
	local chunks = {}
	for i = 1, payload_len do
		chunks[i] = string.char(bytes[N.HEADER_BYTES + i])
	end

	return {
		state = bytes[4],
		ui_session = get16(bytes, 4),
		request_id = get32(bytes, 6),
		fragment_index = get16(bytes, 10),
		fragment_count = get16(bytes, 12),
		payload_len = payload_len,
		revision = get32(bytes, 16),
		slot = get16(bytes, 20),
		flags = bytes[23],
		payload = table.concat(chunks),
	}
end

--- Initialise the measurement worker. Must run after UIParent exists.
function N.init(parent)
	-- The measurer must be laid out, so it is placed on screen but fully
	-- transparent rather than hidden (some builds return 0 width for a hidden
	-- font string).
	measurer = parent:CreateFontString(nil, "BACKGROUND")
	measurer:SetPoint("CENTER", parent, "CENTER")
	measurer:SetAlpha(0)
	measurer:SetFont(N.FONT_PATH:format(1), N.FONT_SIZE, "")

	worker = CreateFrame("Frame")
	worker:Hide()
	worker:SetScript("OnUpdate", N.on_update)
end

--- Queue a read of `slot`. `callback(packet, reason)` runs when finished.
--- `packet` is nil and `reason` explains the failure when the read is stale.
function N.request(slot, callback)
	queue[#queue + 1] = { slot = slot, callback = callback }
	if worker then
		worker:Show()
	end
end

function N.on_update()
	if not active then
		active = table.remove(queue, 1)
		if not active then
			worker:Hide()
			return
		end
		active.stage = "load"
		active.tries = 0
		active.wait = 0
		active.index = 1
		active.widths = {}
	end
	N.step(active)
end

function N.finish(job, packet, reason)
	active = nil
	if job.callback then
		job.callback(packet, reason)
	end
end

function N.step(job)
	if job.stage == "load" then
		-- Trigger the load, then give the client a frame or two to settle.
		measurer:SetFont(N.FONT_PATH:format(job.slot), N.FONT_SIZE, "")
		job.stage = "cal"
		job.wait = 2
		return
	end

	if job.stage == "cal" then
		if job.wait > 0 then
			job.wait = job.wait - 1
			return
		end
		measurer:SetText(N.CAL_LOW .. N.TRAIL)
		local low = measurer:GetStringWidth()
		measurer:SetText(N.CAL_HIGH .. N.TRAIL)
		local high = measurer:GetStringWidth()
		if low > 0 and (high - low) >= MIN_CAL_SPAN then
			job.low, job.high = low, high
			job.stage = "data"
			return
		end
		job.tries = job.tries + 1
		if job.tries > MAX_CAL_TRIES then
			-- Report what was measured: this separates "the font never loaded"
			-- from "the font loaded but its widths are unusable".
			N.last_calibration = string.format(
				"low=%.1f high=%.1f slot=%d",
				low or -1,
				high or -1,
				job.slot
			)
			N.finish(job, nil, "font-not-ready")
		end
		return
	end

	if job.stage == "data" then
		local last = math.min(job.index + GLYPHS_PER_FRAME - 1, N.DATA_GLYPHS)
		for i = job.index, last do
			measurer:SetText(data_char(i) .. N.TRAIL)
			job.widths[i] = measurer:GetStringWidth()
		end
		job.index = last + 1
		if job.index > N.DATA_GLYPHS then
			job.stage = "decode"
		end
		return
	end

	if job.stage == "decode" then
		local bytes = {}
		for i = 1, N.DATA_GLYPHS do
			bytes[i] = decode_byte(job.low, job.high, job.widths[i] or 0)
		end
		local packet, reason = N.parse_packet(bytes)
		if not packet then
			N.finish(job, nil, reason)
			return
		end
		if packet.slot ~= job.slot then
			-- A previously loaded font answered: the client served cached data.
			N.finish(job, nil, "stale")
			return
		end
		N.finish(job, packet, nil)
	end
end

--- Number of reads waiting to be processed.
function N.queue_length()
	return #queue + (active and 1 or 0)
end
