--[[
OCWow :: Main

Wires everything together: the pixel strip, the font-metric receiver, the UI,
slash commands, and the request lifecycle.

Sending a prompt:
  1. transmit phase - the prompt is split into 40-byte fragments and painted on
     the strip, a few passes so a missed capture can recover.
  2. receive phase - control frames advertise the font slot the addon is about
     to load and the reply fragment it wants; after the advertised window the
     slot is loaded and measured.
  3. replies are assembled by fragment index and displayed when complete.

Font slots are consumed monotonically and remembered in SavedVariables, because
the client caches a font after its first load in a given process.
]]

local _, ns = ...
local OCWow = ns or OCWow
local P = OCWow.Protocol
local N = OCWow.Native
local C = OCWow.Context
local U = OCWow.UI

local M = {}
OCWow.Main = M

local TRANSMIT_INTERVAL = 0.25
local RECEIVE_INTERVAL = 0.30

--- Font-bank size created by `ocw install`.
local BANK_SIZE = 4096

-- Defaults so the module never nil-indexes, even if SavedVariables are absent
-- (a fresh install has no OCWowDB file) or ADDON_LOADED was somehow missed.
local db = {
	next_slot = 1,
	context_tier = C.TIER_LIGHT,
	include_context = true,
	transport_enabled = true,
}
local initialized = false
local strip
local ticker
local accum = 0

local state = {
	ui_session = 0,
	next_request_id = 1,
	pending = nil,
	calibrating = false,
	context_tier = C.TIER_LIGHT,
	include_context = true,
}

local TIER_NAMES = { "light", "normal", "full" }

function M.init()
	if initialized then
		return
	end
	initialized = true

	-- The client does not pre-create the SavedVariables table, and it will even
	-- persist `OCWowDB = nil` if the global was nil at logout, so recover from
	-- any non-table value rather than just nil.
	if type(OCWowDB) ~= "table" then
		OCWowDB = {}
	end
	db = OCWowDB

	if not db.next_slot or db.next_slot < 1 then
		db.next_slot = 1
	end
	if db.context_tier == nil then
		db.context_tier = C.TIER_LIGHT
	end
	if db.include_context == nil then
		db.include_context = true
	end
	if db.transport_enabled == nil then
		db.transport_enabled = true
	end

	-- Some client builds omit `math.randomseed`, and an unseeded `math.random`
	-- repeats the same sequence every session, so derive the session id from the
	-- clock instead of the RNG.
	local clock = (time() or 0) * 1000 + math.floor((GetTime() or 0) * 1000)
	state.ui_session = (clock % 65535) + 1
	state.context_tier = db.context_tier
	state.include_context = db.include_context
end

function M.setup()
	-- Idempotent: guarantees db and the session id even if ADDON_LOADED was
	-- missed, since PLAYER_LOGIN always fires.
	M.init()

	strip = P.create_strip(UIParent)
	N.init(UIParent)
	if not db.transport_enabled then
		strip:Hide()
	end
	U.create(M.send_prompt)
	U.set_context_tier(state.context_tier)
	U.on_context_click = M.cycle_context

	ticker = CreateFrame("Frame")
	ticker:SetScript("OnUpdate", function(_, dt)
		accum = accum + dt
		local interval = RECEIVE_INTERVAL
		local pending = state.pending
		if pending and pending.active and pending.phase == "transmit" then
			interval = TRANSMIT_INTERVAL
		end
		if accum >= interval then
			accum = 0
			M.tick()
		end
	end)

	U.add("OCWow ready. /ocw to toggle, /ocw calibrate if the companion cannot find the strip.", 0.7, 0.9, 0.7)
end

-- ---------------------------------------------------------------------------
-- The strip ticker
-- ---------------------------------------------------------------------------

function M.tick()
	if not db.transport_enabled then
		return
	end
	if state.calibrating then
		P.render_strip(strip, P.idle_frame(state.ui_session))
		return
	end
	local pending = state.pending
	if not pending or not pending.active then
		P.render_strip(strip, P.idle_frame(state.ui_session))
		return
	end
	if pending.phase == "transmit" then
		M.tick_transmit(pending)
	else
		M.tick_receive(pending)
	end
end

function M.tick_transmit(pending)
	if pending.frag_index <= #pending.fragments then
		local fragment = pending.fragments[pending.frag_index]
		P.render_strip(
			strip,
			P.encode_prompt(
				state.ui_session,
				pending.request_id,
				pending.frag_index - 1,
				#pending.fragments,
				fragment
			)
		)
		pending.frag_index = pending.frag_index + 1
		return
	end

	pending.frag_index = 1
	pending.pass = pending.pass + 1

	local passes = 1
	if #pending.fragments <= 4 then
		passes = 3
	elseif #pending.fragments <= 12 then
		passes = 2
	end

	if pending.pass >= passes then
		pending.phase = "receive"
		pending.ticks = 0
		pending.wait_ticks = 4
		U.set_status("waiting for reply...")
	end
end

function M.tick_receive(pending)
	if pending.reading then
		return
	end

	-- Re-send a prompt fragment occasionally so a missed frame still starts the job.
	pending.resend = (pending.resend or 0) + 1
	if pending.resend % 12 == 0 and #pending.fragments > 0 then
		pending.resend_index = ((pending.resend_index or 0) % #pending.fragments) + 1
		P.render_strip(
			strip,
			P.encode_prompt(
				state.ui_session,
				pending.request_id,
				pending.resend_index - 1,
				#pending.fragments,
				pending.fragments[pending.resend_index]
			)
		)
		return
	end

	if pending.ticks < pending.wait_ticks then
		P.render_strip(
			strip,
			P.encode_control(
				state.ui_session,
				pending.request_id,
				pending.req_fragment,
				pending.slot,
				pending.wait_ticks * 250,
				P.STATE.WORKING,
				1,
				pending.last_slot or 0
			)
		)
		pending.ticks = pending.ticks + 1
		return
	end

	local slot = pending.slot
	pending.last_slot = slot
	pending.slot = slot + 1
	db.next_slot = pending.slot
	pending.ticks = 0
	pending.reading = true

	if slot > BANK_SIZE then
		U.add("font bank exhausted; restart the game client and the companion", 1, 0.5, 0.3)
		M.finish(pending)
		return
	end

	N.request(slot, function(packet, reason)
		M.on_reply(pending, packet, reason)
	end)
end

-- ---------------------------------------------------------------------------
-- Replies
-- ---------------------------------------------------------------------------

function M.on_reply(pending, packet, reason)
	pending.reading = false
	if not pending.active then
		return
	end

	if not packet then
		pending.stale = (pending.stale or 0) + 1
		if pending.stale >= 10 then
			local detail = N.last_calibration and (" [" .. N.last_calibration .. "]") or ""
			U.add(
				"transport stalled ("
					.. tostring(reason)
					.. ")"
					.. detail
					.. ". Is the companion running, and the strip visible and unobscured?",
				1,
				0.5,
				0.3
			)
			M.finish(pending)
		end
		return
	end

	pending.stale = 0

	if packet.state == P.STATE.RESET then
		db.next_slot = 1
		pending.slot = 1
		pending.req_fragment = 1
		pending.parts = {}
		pending.wait_ticks = 4
		U.set_status("transport reset")
		return
	end

	if packet.state == P.STATE.DONE or packet.state == P.STATE.FAILED then
		if pending.last_revision and pending.last_revision ~= packet.revision then
			pending.parts = {}
		end
		pending.last_revision = packet.revision
		pending.parts[packet.fragment_index] = packet.payload

		if packet.fragment_index < packet.fragment_count then
			pending.req_fragment = packet.fragment_index + 1
			pending.wait_ticks = 4
			U.set_status(string.format("receiving %d/%d...", packet.fragment_index, packet.fragment_count))
			return
		end

		local text = table.concat(pending.parts, "")
		if packet.state == P.STATE.FAILED then
			U.add("opencode: " .. text, 1, 0.5, 0.4)
		else
			U.add("opencode: " .. text, 0.7, 1, 0.75)
		end
		M.finish(pending)
		return
	end

	-- queued / working / streaming
	U.set_status(P.STATE_NAME[packet.state] or "working")
	pending.wait_ticks = math.min(pending.wait_ticks * 2, 20)
	pending.ticks = 0
end

function M.finish(pending)
	pending.active = false
	state.pending = nil
	U.set_status("ready")
end

-- ---------------------------------------------------------------------------
-- Sending
-- ---------------------------------------------------------------------------

function M.send_prompt(text)
	text = text and text:match("^%s*(.-)%s*$") or ""
	if text == "" then
		return
	end

	if not db.transport_enabled then
		U.add("transport is off; use /ocw transport on to enable", 1, 0.6, 0.3)
		return
	end

	local pending = state.pending
	if pending and pending.active then
		U.add("a request is already in flight; wait or use /ocw stop", 1, 0.6, 0.3)
		return
	end

	local is_command = text:sub(1, 1) == "/"
	local body = text
	if state.include_context and not is_command then
		local snapshot = C.snapshot(state.context_tier)
		if snapshot and snapshot ~= "" then
			body = text .. "\n\n" .. P.CONTEXT_OPEN .. "\n" .. snapshot .. "\n" .. P.CONTEXT_CLOSE
		end
	end

	if #body > P.MAX_PROMPT_BYTES then
		U.add(string.format("prompt too long (%d > %d bytes)", #body, P.MAX_PROMPT_BYTES), 1, 0.4, 0.4)
		return
	end

	local fragments = P.fragment_prompt(body)
	local request_id = state.next_request_id
	state.next_request_id = request_id + 1

	state.pending = {
		active = true,
		request_id = request_id,
		fragments = fragments,
		phase = "transmit",
		frag_index = 1,
		pass = 0,
		slot = db.next_slot or 1,
		req_fragment = 1,
		parts = {},
		ticks = 0,
		wait_ticks = 4,
		reading = false,
		stale = 0,
		resend = 0,
	}

	U.add("you: " .. text, 0.7, 0.85, 1)
	U.set_status("sending...")
end

function M.cycle_context()
	state.context_tier = (state.context_tier + 1) % 3
	db.context_tier = state.context_tier
	U.set_context_tier(state.context_tier)
	U.add("game context: " .. TIER_NAMES[state.context_tier + 1], 0.8, 0.8, 0.8)
end

function M.status_text()
	return string.format(
		"session %d, next request %d, next slot %d, context %s",
		state.ui_session,
		state.next_request_id,
		db.next_slot or 1,
		TIER_NAMES[state.context_tier + 1]
	)
end

-- ---------------------------------------------------------------------------
-- Slash commands
-- ---------------------------------------------------------------------------

local function set_calibrating(on)
	state.calibrating = on
	U.set_status(on and "calibrating: the strip is showing a fixed frame" or "ready")
end

local COMMANDS = {
	show = function()
		U.show()
	end,
	hide = function()
		U.hide()
	end,
	toggle = function()
		U.toggle()
	end,
	ctx = function()
		M.cycle_context()
	end,
	calibrate = function()
		set_calibrating(not state.calibrating)
	end,
	status = function()
		U.add(M.status_text(), 0.85, 0.85, 0.85)
	end,
	transport = function(_, rest)
		if rest == "off" then
			db.transport_enabled = false
			if strip then
				strip:Hide()
			end
			U.add("transport off: strip hidden, no font reads", 0.9, 0.8, 0.4)
		elseif rest == "on" then
			db.transport_enabled = true
			if strip then
				strip:Show()
			end
			U.add("transport on", 0.7, 0.9, 0.7)
		else
			U.add("usage: /ocw transport on|off", 0.8, 0.8, 0.8)
		end
	end,
	context = function(_, rest)
		if rest == "on" then
			state.include_context = true
			db.include_context = true
			U.add("context: on", 0.8, 0.8, 0.8)
		elseif rest == "off" then
			state.include_context = false
			db.include_context = false
			U.add("context: off", 0.8, 0.8, 0.8)
		else
			U.add("usage: /ocw context on|off", 0.8, 0.8, 0.8)
		end
	end,
}

function M.slash(msg)
	msg = msg or ""
	local command, rest = msg:match("^%s*(%S*)%s*(.-)%s*$")
	command = (command or ""):lower()

	if command == "" then
		U.toggle()
		return
	end

	local handler = COMMANDS[command]
	if handler then
		handler(command, rest)
		return
	end

	if command == "new" or command == "stop" or command == "help" or command == "model" then
		M.send_prompt("/" .. command .. (rest ~= "" and (" " .. rest) or ""))
		return
	end

	M.send_prompt(msg)
end

-- ---------------------------------------------------------------------------
-- Bootstrap
-- ---------------------------------------------------------------------------

local function report_failure(stage, err)
	local message = string.format("OCWow: %s failed: %s", stage, tostring(err))
	print(message)
	if U and U.add then
		pcall(U.add, message, 1, 0.4, 0.4)
	end
end

local events = CreateFrame("Frame")
events:RegisterEvent("ADDON_LOADED")
events:RegisterEvent("PLAYER_LOGIN")
events:SetScript("OnEvent", function(_, event, arg1)
	if event == "ADDON_LOADED" and arg1 == "OCWow" then
		local ok, err = pcall(M.init)
		if not ok then
			report_failure("init", err)
		end
	elseif event == "PLAYER_LOGIN" then
		local ok, err = pcall(M.setup)
		if not ok then
			report_failure("setup", err)
		end
	end
end)

SLASH_OCWOW1 = "/ocw"
SLASH_OCWOW2 = "/ocwow"
SlashCmdList["OCWOW"] = function(msg)
	M.slash(msg)
end
