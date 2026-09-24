--[[
OCWow :: Main

Wires the transport, the tabbed UI and the request lifecycle together.

  * OUT: a message is drawn as a strip of coloured cells in the top-left of the
    game window; the companion screen-captures that corner and decodes it.
  * IN: the companion writes the latest state of every chat into a bank of
    load-on-demand addons; we load a fresh one and read the global it defines.
  * Signals: `PlaySoundFile` reports whether a file will play, so a pre-made
    empty `.wav` is a one-shot flag the companion raises and we poll for free.

Each tab is an independent OpenCode session with its own transcript.
]]

local ADDON_NAME, ns = ...
-- Every file must use the addon namespace table (`...`), not the global: the
-- files that set fields on it shadow the global, so reading `OCWow` here would
-- see an empty table.
local OCWow = ns
if type(OCWow) ~= "table" then
	print("OCWow: the addon namespace table is missing; check the TOC file order")
	return
end
_G.OCWow = OCWow

local Codec = OCWow.Codec
if type(Codec) ~= "table" then
	print("OCWow: Codec.lua did not load; check the TOC file order")
	return
end

local DEFAULT_CWD = ""
local MAX_TABS = 8

local SLOT_COUNT = 200
local SLOT_PREFIX = "OCWow_S"
local STRIP_TRIES = 3
local STRIP_SECONDS = 40
local TICK_SECONDS = 2
local POLL_SCHEDULE = { 5, 10, 16, 24, 34, 46, 60, 80, 100, 130, 160, 200, 240, 300 }
local POLL_TAIL = 60

local CELL = Codec.CELL_PX
local CELLS_PER_ROW = Codec.CELLS_PER_ROW
local MAX_ROWS = Codec.MAX_ROWS
local RS, US = Codec.RECORD_SEP, Codec.FIELD_SEP

local db
local ui = {}
-- Transport state for this UI session: outbound[id] = { tab, flags, text, sentAt, acked }
local run = { outbound = {} }

---------------------------------------------------------------------------
-- Helpers
---------------------------------------------------------------------------

local function Trim(s)
	return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function FmtDur(sec)
	sec = math.floor(sec or 0)
	if sec < 60 then
		return sec .. "s"
	end
	return math.floor(sec / 60) .. "m" .. string.format("%02d", sec % 60) .. "s"
end

local function LoadAddOn(name)
	if C_AddOns and C_AddOns.LoadAddOn then
		return C_AddOns.LoadAddOn(name)
	end
	if LoadAddOn then
		return LoadAddOn(name)
	end
end

local function IsAddOnLoaded(name)
	if C_AddOns and C_AddOns.IsAddOnLoaded then
		return C_AddOns.IsAddOnLoaded(name)
	end
	if IsAddOnLoaded then
		return IsAddOnLoaded(name)
	end
	return false
end

local function SlotName(index)
	return string.format("%s%03d", SLOT_PREFIX, index)
end

local function SlotNumber(id)
	return ((id - 1) % SLOT_COUNT) + 1
end

local function NewId()
	return string.format("%x%04x%04x", time() % 0xFFFFFF, math.floor(GetTime() * 1000) % 0xFFFF, math.floor(GetTime() * 1000000) % 0xFFFF)
end

local function InitDB()
	if type(OCWowDB) ~= "table" then
		OCWowDB = {}
	end
	db = OCWowDB
	if not db.seq then
		db.seq = 0
	end
	if type(db.tab_labels) ~= "table" then
		db.tab_labels = {}
	end
	if not db.tab_count or db.tab_count < 1 then
		db.tab_count = 1
	end
	if db.tab_count > MAX_TABS then
		db.tab_count = MAX_TABS
	end
	if db.context == nil then
		db.context = true
	end
	if db.contextTier == nil then
		db.contextTier = OCWow.Context.TIER_LIGHT
	end
	if db.transport == nil then
		db.transport = true
	end
	if not db.session then
		db.session = NewId()
	end
end

---------------------------------------------------------------------------
-- The pixel strip (out)
---------------------------------------------------------------------------

local strip
local cellPool = {}

local function EnsureStrip()
	if strip then
		return strip
	end
	strip = CreateFrame("Frame", "OCWowStrip", UIParent)
	strip:SetFrameStrata("TOOLTIP")
	strip:SetFrameLevel(10000)
	-- Scale so that one UI unit is exactly one physical pixel (Blizzard's
	-- PixelUtil trick), so the strip is a fixed size on screen regardless of the
	-- player's UI scale.
	local physicalHeight = 1080
	if GetPhysicalScreenSize then
		local _, height = GetPhysicalScreenSize()
		physicalHeight = height or physicalHeight
	end
	if strip.SetIgnoreParentScale then
		strip:SetIgnoreParentScale(true)
	end
	strip:SetScale(768 / physicalHeight)
	strip:SetPoint("TOPLEFT", UIParent, "TOPLEFT", 0, 0)
	strip:SetSize(CELLS_PER_ROW * CELL, MAX_ROWS * CELL)
	strip:Hide()
	return strip
end

local function HideStrip()
	if strip then
		strip:Hide()
	end
end

local function ShowStrip(id, payload)
	local cells = Codec.EncodeFrame(id % 65536, payload)
	local rows = math.ceil(#cells / CELLS_PER_ROW)
	local total = rows * CELLS_PER_ROW
	local frame = EnsureStrip()

	for i = 1, total do
		local texture = cellPool[i]
		if not texture then
			texture = frame:CreateTexture(nil, "OVERLAY")
			texture:SetSize(CELL, CELL)
			local column = (i - 1) % CELLS_PER_ROW
			local row = math.floor((i - 1) / CELLS_PER_ROW)
			texture:SetPoint("TOPLEFT", frame, "TOPLEFT", column * CELL, -row * CELL)
			cellPool[i] = texture
		end
		local r, g, b = Codec.CellColor(cells[i] or 0)
		texture:SetColorTexture(r, g, b, 1)
		texture:Show()
	end
	for i = total + 1, #cellPool do
		cellPool[i]:Hide()
	end
	frame:Show()
end

--- Redraw the strip from every outbound message the companion hasn't acknowledged.
local function RefreshStrip()
	local ids = {}
	for id, record in pairs(run.outbound) do
		if not record.acked then
			ids[#ids + 1] = id
		end
	end
	if #ids == 0 then
		HideStrip()
		return
	end
	table.sort(ids)

	local parts, size = {}, 0
	local latest = ids[#ids]
	for i = #ids, 1, -1 do
		local record = run.outbound[ids[i]]
		local fields = {
			db.session,
			tostring(record.tab),
			tostring(ids[i]),
			record.cwd or "",
			record.flags or "",
			record.name or "",
		}
		if record.ctx ~= nil then
			fields[5] = (fields[5] == "" and "c") or (fields[5] .. ";c")
			fields[#fields + 1] = record.ctx
		end
		fields[#fields + 1] = record.text
		local encoded = table.concat(fields, US):gsub("[" .. RS .. US .. "]", " ")
		if size + #encoded + 1 > Codec.MAX_PAYLOAD then
			break
		end
		table.insert(parts, 1, encoded)
		size = size + #encoded + 1
	end
	ShowStrip(latest, table.concat(parts, RS))
end

---------------------------------------------------------------------------
-- Signals and slots (in)
---------------------------------------------------------------------------

local signalAvailable = type(PlaySoundFile) == "function"

local function SoundValid(path)
	if not signalAvailable then
		return false
	end
	local ok, willPlay, handle = pcall(PlaySoundFile, path, "Master")
	if not ok then
		signalAvailable = false
		return false
	end
	if willPlay and handle then
		pcall(StopSound, handle)
	end
	return willPlay and true or false
end

local function SignalPath(kind, index)
	return string.format("Interface\\AddOns\\OCWow\\%s\\%03d.wav", kind, index)
end

local function CheckSignal(kind, id)
	return SoundValid(SignalPath(kind, SlotNumber(id)))
end

--- Prove the sound channel distinguishes empty from valid files on this client.
local function SelfTestSignals()
	if not signalAvailable then
		return
	end
	local emptyLooksValid = SoundValid("Interface\\AddOns\\OCWow\\ctl\\empty.wav")
	local validLooksValid = SoundValid("Interface\\AddOns\\OCWow\\ctl\\valid.wav")
	if emptyLooksValid or not validLooksValid then
		signalAvailable = false
	end
end

local function FreeSlot()
	for i = 1, SLOT_COUNT do
		local name = SlotName(i)
		if not IsAddOnLoaded(name) then
			return name
		end
	end
end

local function ScheduleNextPoll()
	local index = (run.polls or 0) + 1
	local seconds = POLL_SCHEDULE[index]
	if not seconds then
		seconds = POLL_SCHEDULE[#POLL_SCHEDULE] + POLL_TAIL * (index - #POLL_SCHEDULE)
	end
	run.nextPollAt = (run.sentAt or GetTime()) + seconds
end

local function FindTab(id)
	for _, tab in pairs(run.tabs or {}) do
		if tab.id == id then
			return tab
		end
	end
end

local Finish -- forward declaration

local function ApplyReplies(replies)
	for _, reply in ipairs(replies or {}) do
		local tab = FindTab(reply.tab)
		if tab and tab.pendingId == reply.request then
			if reply.status == "done" then
				Finish(tab, "opencode", reply.text or "")
			elseif reply.status == "error" then
				Finish(tab, "system", "Bridge error: " .. tostring(reply.text))
			else
				tab.progress = reply.text
			end
		end
	end
end

local function TryLoadSlot(why)
	local name = FreeSlot()
	if not name then
		run.slotsExhausted = true
		return
	end
	OCWow_SlotData = nil
	local loaded = LoadAddOn(name)
	if not loaded then
		run.slotsMissing = true
		return
	end
	run.polls = (run.polls or 0) + 1
	ScheduleNextPoll()
	local data = OCWow_SlotData
	if type(data) == "table" then
		if type(data.now) == "number" then
			run.bridgeSeen = GetTime() - (time() - data.now)
		end
		ApplyReplies(data.replies)
	end
	if why == "signal" and run.signalUnreliable == nil then
		run.signalUnreliable = false
	end
	OCWow.Render()
end

---------------------------------------------------------------------------
-- Sending
---------------------------------------------------------------------------

local function ContextToSend(room)
	local text = db.context and OCWow.Context.snapshot(db.contextTier) or ""
	if text == (run.contextSent or "") then
		return nil
	end
	if room and #text > room then
		return nil
	end
	return text
end

function OCWow.Send(text, newSession)
	local tab = OCWow.ActiveTab()
	if not tab then
		return
	end
	text = Trim(text or "")
	if text == "" then
		return
	end
	if tab.pendingId then
		OCWow.AddHistory(tab, "system", "This tab is still waiting for a reply.")
		OCWow.Render()
		return
	end
	if not db.transport then
		OCWow.AddHistory(tab, "system", "Transport is off (/ocw transport on).")
		OCWow.Render()
		return
	end

	local limit = Codec.MAX_PAYLOAD - 400
	if #text > limit then
		OCWow.AddHistory(tab, "system", "That message is too long (" .. #text .. " > " .. limit .. " bytes).")
		OCWow.Render()
		return
	end

	local context = ContextToSend(limit - #text)

	db.seq = db.seq + 1
	local id = db.seq
	local flags = {}
	if newSession or tab.newSession then
		flags[#flags + 1] = "n"
		tab.newSession = nil
	end

	run.outbound[id] = {
		tab = tab.id,
		cwd = tab.cwd or DEFAULT_CWD,
		flags = table.concat(flags, ";"),
		name = tab.label,
		ctx = context,
		text = text,
		sentAt = GetTime(),
	}
	tab.pendingId = id
	tab.draft = nil
	run.sentAt = GetTime()
	run.polls = 0
	ScheduleNextPoll()
	OCWow.AddHistory(tab, "user", text, id)
	RefreshStrip()
	OCWow.Render()
end

--- A record with no text that just announces this session token, so the
--- companion can ack, refresh the slots and offer a restore.
function OCWow.SayHello()
	if not db or not db.transport then
		return
	end
	db.seq = db.seq + 1
	local tab = OCWow.ActiveTab()
	run.outbound[db.seq] = {
		tab = tab and tab.id or 0,
		cwd = tab and tab.cwd or "",
		flags = "h",
		name = tab and tab.label or "",
		ctx = db.context and OCWow.Context.snapshot(db.contextTier) or "",
		text = "",
		sentAt = GetTime(),
		hello = true,
	}
	RefreshStrip()
	OCWow.Render()
end

--- Put the active tab's pending message back on the strip.
function OCWow.Resend()
	local tab = OCWow.ActiveTab()
	if not tab or not tab.pendingId then
		return
	end
	local text
	for i = #tab.history, 1, -1 do
		if tab.history[i].id == tab.pendingId then
			text = tab.history[i].text
			break
		end
	end
	if not text then
		return
	end
	run.outbound[tab.pendingId] = {
		tab = tab.id,
		cwd = tab.cwd or "",
		flags = "",
		name = tab.label,
		text = text,
		sentAt = GetTime(),
	}
	run.sentAt = GetTime()
	run.polls = 0
	ScheduleNextPoll()
	RefreshStrip()
	OCWow.Render()
end

---------------------------------------------------------------------------
-- Tabs
---------------------------------------------------------------------------

function OCWow.EnsureTab(id, label)
	run.tabs = run.tabs or {}
	local tab = run.tabs[id]
	if not tab then
		tab = {
			id = id,
			label = label or db.tab_labels[id] or tostring(id),
			cwd = DEFAULT_CWD,
			history = {},
			pendingId = nil,
			unread = 0,
			created = time(),
		}
		run.tabs[id] = tab
		ui.create_tab(id, tab.label)
	end
	return tab
end

function OCWow.ActiveTab()
	run.tabs = run.tabs or {}
	return run.tabs[run.activeTab or 1]
end

function OCWow.NewTab()
	local id = 1
	while run.tabs and run.tabs[id] do
		id = id + 1
	end
	if id > MAX_TABS then
		OCWow.AddHistory(OCWow.ActiveTab(), "system", "Tab limit reached (" .. MAX_TABS .. ").")
		OCWow.Render()
		return
	end
	OCWow.EnsureTab(id)
	OCWow.SwitchTab(id)
	db.tab_count = math.max(db.tab_count or 1, id)
end

function OCWow.SwitchTab(id)
	if not run.tabs or not run.tabs[id] then
		return
	end
	run.activeTab = id
	ui.set_active_tab(id)
	OCWow.Render()
end

function OCWow.CloseTab(id)
	local tab = run.tabs and run.tabs[id]
	if not tab then
		return
	end
	if ui.tab_count() <= 1 then
		OCWow.AddHistory(tab, "system", "Cannot close the last tab.")
		OCWow.Render()
		return
	end
	-- Tell the companion to forget this tab's session.
	db.seq = db.seq + 1
	run.outbound[db.seq] = {
		tab = id,
		cwd = tab.cwd or "",
		flags = "d",
		name = tab.label,
		text = "",
		sentAt = GetTime(),
	}
	RefreshStrip()

	run.tabs[id] = nil
	db.tab_labels[id] = nil
	ui.close_tab(id)
	if run.activeTab == id then
		local ids = ui.tab_ids()
		OCWow.SwitchTab(ids[1] or 1)
	end
end

function OCWow.RenameTab(label)
	local tab = OCWow.ActiveTab()
	if not tab then
		return
	end
	label = Trim(label or "")
	if label == "" then
		label = tostring(tab.id)
	end
	tab.label = label
	db.tab_labels[tab.id] = label
	ui.set_tab_label(tab.id, label)
end

local ROLE_STYLE = {
	user = { prefix = "you: ", r = 0.7, g = 0.85, b = 1.0 },
	opencode = { prefix = "opencode: ", r = 0.7, g = 1.0, b = 0.75 },
	system = { prefix = "", r = 0.8, g = 0.8, b = 0.8 },
}

--- Append to a tab's history *and* its transcript, so the panel shows it.
function OCWow.AddHistory(tab, role, text, id)
	if not tab then
		return
	end
	tab.history[#tab.history + 1] = { role = role, text = text, id = id, t = time() }
	while #tab.history > 200 do
		table.remove(tab.history, 1)
	end

	local style = ROLE_STYLE[role] or ROLE_STYLE.system
	if ui and ui.add_to then
		ui.add_to(tab.id, style.prefix .. tostring(text or ""), style.r, style.g, style.b)
	end
	if not (ui and ui.is_shown and ui.is_shown() and run.activeTab == tab.id) then
		tab.unread = (tab.unread or 0) + 1
		if ui and ui.set_tab_unread then
			ui.set_tab_unread(tab.id, true)
		end
	end
end

Finish = function(tab, role, text)
	OCWow.AddHistory(tab, role, text, tab.pendingId)
	tab.pendingId = nil
	tab.progress = nil
	run.bridgeSeen = GetTime()
	OCWow.Render()
end

---------------------------------------------------------------------------
-- Ticking
---------------------------------------------------------------------------

function OCWow.Tick()
	if not db then
		return
	end
	local now = GetTime()

	-- Acks: the companion read the message, so it can leave the strip.
	local changed = false
	for id, record in pairs(run.outbound) do
		if not record.acked and CheckSignal("ack", id) then
			record.acked = true
			if record.ctx ~= nil then
				run.contextSent = record.ctx
			end
			changed = true
		end
		if record.hello and not record.acked and run.bridgeSeen and run.bridgeSeen >= record.sentAt + 2 then
			record.acked = true
			changed = true
		end
		if record.acked then
			run.outbound[id] = nil
			changed = true
		elseif record.hello and now - record.sentAt >= 20 then
			run.outbound[id] = nil
			changed = true
		elseif now - record.sentAt >= STRIP_SECONDS then
			record.tries = (record.tries or 1) + 1
			if record.tries <= STRIP_TRIES then
				record.sentAt = now
				changed = true
			else
				run.outbound[id] = nil
				run.pixelFailed = true
				changed = true
				local tab = FindTab(record.tab)
				if tab then
					OCWow.AddHistory(tab, "system", "The bridge didn't see that message. Is it running? (/ocw resend)")
				end
			end
		end
	end
	if changed then
		RefreshStrip()
		OCWow.Render()
	end

	if not db.transport then
		return
	end

	-- Readiness: load a slot as soon as a reply is ready, otherwise on schedule.
	if run.nextPollAt and now >= run.nextPollAt then
		TryLoadSlot("schedule")
	end
	for _, tab in pairs(run.tabs or {}) do
		if tab.pendingId and CheckSignal("sig", tab.pendingId) then
			TryLoadSlot("signal")
			break
		end
	end
end

---------------------------------------------------------------------------
-- Rendering
---------------------------------------------------------------------------

function OCWow.Render()
	if not ui.set_status then
		return
	end
	local tab = OCWow.ActiveTab()
	local parts = {}
	if run.pixelFailed then
		parts[#parts + 1] = "bridge not answering"
	elseif run.slotsMissing then
		parts[#parts + 1] = "slots missing (run ocw install)"
	elseif run.slotsExhausted then
		parts[#parts + 1] = "slot pool used up - /reload to free it"
	end
	if tab and tab.pendingId then
		local elapsed = run.sentAt and (GetTime() - run.sentAt) or 0
		parts[#parts + 1] = "working " .. FmtDur(elapsed)
	end
	if #parts == 0 then
		parts[#parts + 1] = "ready"
	end
	ui.set_status(table.concat(parts, " - "))
end

---------------------------------------------------------------------------
-- Slash commands
---------------------------------------------------------------------------

local COMMANDS = {
	show = function()
		ui.show()
	end,
	hide = function()
		ui.hide()
	end,
	toggle = function()
		ui.toggle()
	end,
	test = function()
		OCWow.SayHello()
		OCWow.AddHistory(OCWow.ActiveTab(), "system", "Strip shown for calibration; run `ocw probe` now.")
		OCWow.Render()
	end,
	resend = function()
		OCWow.Resend()
	end,
	newtab = function()
		OCWow.NewTab()
	end,
	closetab = function()
		OCWow.CloseTab(run.activeTab)
	end,
	tab = function(_, rest)
		local id = tonumber(rest)
		if id and run.tabs and run.tabs[id] then
			OCWow.SwitchTab(id)
		else
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "usage: /ocw tab <number>")
		end
	end,
	rename = function(_, rest)
		OCWow.RenameTab(rest)
	end,
	ctx = function()
		db.contextTier = (db.contextTier + 1) % 3
		ui.set_context_tier(db.contextTier)
		OCWow.AddHistory(OCWow.ActiveTab(), "system", "game context: " .. ({ "light", "normal", "full" })[db.contextTier + 1])
	end,
	context = function(_, rest)
		if rest == "off" then
			db.context = false
			run.contextSent = ""
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "context off")
		elseif rest == "on" then
			db.context = true
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "context on")
		else
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "context: " .. (db.context and "on" or "off"))
		end
	end,
	transport = function(_, rest)
		if rest == "off" then
			db.transport = false
			HideStrip()
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "transport off")
		elseif rest == "on" then
			db.transport = true
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "transport on")
		else
			OCWow.AddHistory(OCWow.ActiveTab(), "system", "transport: " .. (db.transport and "on" or "off"))
		end
	end,
	status = function()
		local tab = OCWow.ActiveTab()
		local lines = {
			"session " .. tostring(db.session),
			"tab " .. tostring(tab and tab.id or "?") .. " (" .. tostring(tab and tab.label or "?") .. ")",
			"next message id " .. tostring(db.seq),
			"signal channel " .. (signalAvailable and "on" or "off"),
			"slots missing " .. tostring(run.slotsMissing or false) .. ", exhausted " .. tostring(run.slotsExhausted or false),
			"bridge seen " .. (run.bridgeSeen and FmtDur(GetTime() - run.bridgeSeen) .. " ago" or "never"),
			"context " .. (db.context and ("on (" .. ({ "light", "normal", "full" })[db.contextTier + 1] .. ")") or "off"),
			"transport " .. (db.transport and "on" or "off"),
		}
		OCWow.AddHistory(tab, "system", table.concat(lines, "\n"))
		OCWow.Render()
	end,
}

function OCWow.Slash(msg)
	msg = msg or ""
	local command, rest = msg:match("^%s*(%S*)%s*(.-)%s*$")
	command = (command or ""):lower()

	if command == "" then
		ui.toggle()
		return
	end
	local handler = COMMANDS[command]
	if handler then
		handler(command, rest)
		OCWow.Render()
		return
	end
	if command == "new" or command == "stop" or command == "help" or command == "model" then
		OCWow.Send("/" .. command .. (rest ~= "" and (" " .. rest) or ""))
		return
	end
	OCWow.Send(msg)
end

---------------------------------------------------------------------------
-- Bootstrap
---------------------------------------------------------------------------

local function ReportFailure(stage, err)
	local message = string.format("OCWow: %s failed: %s", stage, tostring(err))
	print(message)
	if ui and ui.add_to then
		pcall(ui.add_to, run.activeTab or 1, message, 1, 0.4, 0.4)
	end
end

function OCWow.Setup()
	InitDB()
	run.activeTab = 1
	SelfTestSignals()
	ui = OCWow.UI
	ui.create({
		on_send = function(text)
			OCWow.Send(text)
		end,
		on_new_tab = OCWow.NewTab,
		on_select_tab = OCWow.SwitchTab,
	})
	ui.set_context_tier(db.contextTier)

	for id = 1, db.tab_count do
		OCWow.EnsureTab(id)
	end
	OCWow.SwitchTab(1)

	local ticker = CreateFrame("Frame")
	ticker:SetScript("OnUpdate", function()
		if (OCWow.nextTick or 0) > GetTime() then
			return
		end
		OCWow.nextTick = GetTime() + TICK_SECONDS
		OCWow.Tick()
	end)

	OCWow.AddHistory(OCWow.ActiveTab(), "system", "OCWow ready. Each tab is its own session; + opens another.")
	OCWow.SayHello()
end

local events = CreateFrame("Frame")
events:RegisterEvent("ADDON_LOADED")
events:RegisterEvent("PLAYER_LOGIN")
events:SetScript("OnEvent", function(_, event, arg1)
	if event == "ADDON_LOADED" and arg1 == ADDON_NAME then
		local ok, err = pcall(InitDB)
		if not ok then
			ReportFailure("init", err)
		end
	elseif event == "PLAYER_LOGIN" then
		local ok, err = pcall(OCWow.Setup)
		if not ok then
			ReportFailure("setup", err)
		end
	end
end)

SLASH_OCWOW1 = "/ocw"
SLASH_OCWOW2 = "/ocwow"
SlashCmdList["OCWOW"] = function(msg)
	OCWow.Slash(msg)
end

SLASH_OCWOWAI1 = "/ai"
SlashCmdList["OCWOWAI"] = function(msg)
	OCWow.Send(msg)
end
