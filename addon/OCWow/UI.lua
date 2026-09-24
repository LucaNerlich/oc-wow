--[[
OCWow :: UI

A small, self-contained panel with one transcript per session ("tab").

  * a tab bar across the top; click to switch, right-click to close
  * a `+` button to open another session
  * a transcript, input box and action buttons for the active tab
  * a status line shared by all tabs

Deliberately plain so it renders on any client build.
]]

local _, ns = ...
local OCWow = ns or OCWow

local U = {}
OCWow.UI = U

local panel
local status
local context_button
local add_button
local on_send
local on_new_tab
local on_select_tab

--- Tab id -> { button, frame, background, label, unread }
local tabs = {}
--- Tab ids in display order.
local order = {}
local active_id

local TAB_WIDTH = 66
local TAB_HEIGHT = 20
local TAB_GAP = 2
local TAB_TOP = -26
local MAX_TABS = 8

local TIER_LABEL = {
	[0] = "context: light",
	[1] = "context: normal",
	[2] = "context: full",
}

local function tab_x(index)
	return 16 + (index - 1) * (TAB_WIDTH + TAB_GAP)
end

local function layout()
	for index, id in ipairs(order) do
		local tab = tabs[id]
		if tab then
			tab.button:SetPoint("TOPLEFT", panel, "TOPLEFT", tab_x(index), TAB_TOP)
		end
	end
	if add_button then
		add_button:SetPoint("TOPLEFT", panel, "TOPLEFT", tab_x(#order + 1), TAB_TOP)
		if #order >= MAX_TABS then
			add_button:Hide()
		else
			add_button:Show()
		end
	end
end

local function paint_tabs()
	for id, tab in pairs(tabs) do
		local label = tab.label or tostring(id)
		if tab.unread and id ~= active_id then
			label = label .. "*"
		end
		tab.button:SetText(label)
		if id == active_id then
			tab.background:SetColorTexture(0.25, 0.45, 0.75, 0.85)
		elseif tab.unread then
			tab.background:SetColorTexture(0.35, 0.28, 0.12, 0.8)
		else
			tab.background:SetColorTexture(0.08, 0.08, 0.10, 0.7)
		end
	end
end

local function button(parent, text, width, x, y, onClick)
	local b = CreateFrame("Button", nil, parent, "UIPanelButtonTemplate")
	b:SetSize(width, 22)
	b:SetPoint("BOTTOMLEFT", parent, "BOTTOMLEFT", x, y)
	b:SetText(text)
	b:SetScript("OnClick", onClick)
	return b
end

--- Create a tab: a button in the bar and its own transcript frame.
function U.create_tab(id, label)
	if tabs[id] then
		return
	end

	local tab_button = CreateFrame("Button", nil, panel)
	tab_button:SetSize(TAB_WIDTH, TAB_HEIGHT)
	tab_button:SetNormalFontObject(GameFontNormalSmall)
	tab_button:SetText(label or tostring(id))
	tab_button:RegisterForClicks("LeftButtonUp", "RightButtonUp")

	local background = tab_button:CreateTexture(nil, "BACKGROUND")
	background:SetAllPoints()
	background:SetColorTexture(0.08, 0.08, 0.10, 0.7)
	tab_button:SetScript("OnClick", function(_, mouseButton)
		if mouseButton == "RightButton" then
			U.close_tab(id)
		elseif on_select_tab then
			on_select_tab(id)
		end
	end)

	local frame = CreateFrame("ScrollingMessageFrame", nil, panel)
	frame:SetPoint("TOPLEFT", panel, "TOPLEFT", 16, -50)
	frame:SetPoint("BOTTOMRIGHT", panel, "BOTTOMRIGHT", -16, 96)
	frame:SetFontObject(ChatFontNormal)
	frame:SetFading(false)
	frame:SetMaxLines(800)
	frame:SetJustifyH("LEFT")
	frame:SetHyperlinksEnabled(false)
	frame:EnableMouseWheel(true)
	frame:SetScript("OnMouseWheel", function(self, delta)
		if delta > 0 then
			self:ScrollUp()
		else
			self:ScrollDown()
		end
	end)
	frame:Hide()

	tabs[id] = {
		button = tab_button,
		frame = frame,
		background = background,
		label = label or tostring(id),
		unread = false,
	}
	order[#order + 1] = id
	layout()
	paint_tabs()
end

--- Remove a tab and its transcript.
function U.close_tab(id)
	local tab = tabs[id]
	if not tab then
		return
	end
	tab.frame:Hide()
	tab.button:Hide()
	tabs[id] = nil
	for index, value in ipairs(order) do
		if value == id then
			table.remove(order, index)
			break
		end
	end
	layout()
end

function U.tab_ids()
	local ids = {}
	for index, id in ipairs(order) do
		ids[index] = id
	end
	return ids
end

function U.tab_count()
	return #order
end

function U.set_active_tab(id)
	if not tabs[id] then
		return
	end
	for other, tab in pairs(tabs) do
		if other == id then
			tab.frame:Show()
			tab.unread = false
		else
			tab.frame:Hide()
		end
	end
	active_id = id
	paint_tabs()
end

function U.active_tab()
	return active_id
end

function U.set_tab_label(id, label)
	local tab = tabs[id]
	if tab then
		tab.label = label
		paint_tabs()
	end
end

function U.set_tab_unread(id, unread)
	local tab = tabs[id]
	if tab then
		tab.unread = unread and true or false
		paint_tabs()
	end
end

--- Append a message to a specific tab's transcript.
function U.add_to(id, text, r, g, b)
	local tab = tabs[id]
	if not tab then
		return
	end
	tab.frame:AddMessage(text or "", r or 1, g or 1, b or 1)
end

--- Append a message to the active tab.
function U.add(text, r, g, b)
	if active_id then
		U.add_to(active_id, text, r, g, b)
	end
end

--- Create the panel. `callbacks` supplies on_send, on_new_tab, on_select_tab.
function U.create(callbacks)
	callbacks = callbacks or {}
	on_send = callbacks.on_send
	on_new_tab = callbacks.on_new_tab
	on_select_tab = callbacks.on_select_tab

	local ok, frame = pcall(CreateFrame, "Frame", "OCWowPanel", UIParent, "BasicFrameTemplateWithInset")
	if not ok or not frame then
		local ok_backdrop, backdrop = pcall(CreateFrame, "Frame", "OCWowPanel", UIParent, "BackdropTemplate")
		if ok_backdrop and backdrop and backdrop.SetBackdrop then
			backdrop:SetBackdrop({
				bgFile = "Interface\\DialogFrame\\UI-DialogBox-Background",
				edgeFile = "Interface\\DialogFrame\\UI-DialogBox-Border",
				tile = true,
				tileSize = 32,
				edgeSize = 32,
				insets = { left = 8, right = 8, top = 8, bottom = 8 },
			})
			backdrop:SetBackdropColor(0, 0, 0, 0.9)
			frame = backdrop
		else
			frame = CreateFrame("Frame", "OCWowPanel", UIParent)
		end
	end

	panel = frame
	panel:SetSize(520, 380)
	panel:SetPoint("CENTER")
	panel:SetMovable(true)
	panel:EnableMouse(true)
	panel:RegisterForDrag("LeftButton")
	panel:SetScript("OnDragStart", panel.StartMoving)
	panel:SetScript("OnDragStop", panel.StopMovingOrSizing)
	panel:SetFrameStrata("DIALOG")
	panel:SetToplevel(true)
	panel:Hide()

	if panel.TitleText then
		panel.TitleText:SetText("OCWow")
	end

	local title = panel:CreateFontString(nil, "OVERLAY", "GameFontNormal")
	title:SetPoint("TOP", panel, "TOP", 0, -6)
	title:SetText("OCWow - OpenCode")

	add_button = CreateFrame("Button", nil, panel)
	add_button:SetSize(24, TAB_HEIGHT)
	add_button:SetNormalFontObject(GameFontNormalSmall)
	add_button:SetText("+")
	add_button:SetScript("OnClick", function()
		if on_new_tab then
			on_new_tab()
		end
	end)

	local input = CreateFrame("EditBox", "OCWowInput", panel, "InputBoxTemplate")
	input:SetPoint("BOTTOMLEFT", panel, "BOTTOMLEFT", 16, 62)
	input:SetSize(392, 24)
	input:SetAutoFocus(false)
	input:SetMaxLetters(2000)
	input:SetScript("OnEnterPressed", function(self)
		local text = self:GetText()
		self:SetText("")
		if text and text ~= "" and on_send then
			on_send(text)
		end
	end)
	input:SetScript("OnEscapePressed", function(self)
		self:ClearFocus()
	end)
	panel.input = input

	button(panel, "Send", 80, 412, 62, function()
		local text = panel.input:GetText()
		panel.input:SetText("")
		if text and text ~= "" and on_send then
			on_send(text)
		end
	end)

	context_button = button(panel, TIER_LABEL[0], 110, 16, 34, function()
		if U.on_context_click then
			U.on_context_click()
		end
	end)

	button(panel, "New session", 100, 132, 34, function()
		if on_send then
			on_send("/new")
		end
	end)

	button(panel, "Stop", 60, 236, 34, function()
		if on_send then
			on_send("/stop")
		end
	end)

	button(panel, "Help", 60, 300, 34, function()
		if on_send then
			on_send("/help")
		end
	end)

	button(panel, "Hide", 60, 364, 34, function()
		U.hide()
	end)

	status = panel:CreateFontString(nil, "OVERLAY", "GameFontHighlightSmall")
	status:SetPoint("BOTTOMLEFT", panel, "BOTTOMLEFT", 16, 12)
	status:SetPoint("BOTTOMRIGHT", panel, "BOTTOMRIGHT", -16, 12)
	status:SetJustifyH("LEFT")
	status:SetText("ready")

	return panel
end

--- Set the status line.
function U.set_status(text)
	if status then
		status:SetText(text or "")
	end
end

--- Update the context button label for a tier.
function U.set_context_tier(tier)
	if context_button then
		context_button:SetText(TIER_LABEL[tier] or TIER_LABEL[0])
	end
end

function U.show()
	if panel then
		panel:Show()
		panel.input:SetFocus()
	end
end

function U.hide()
	if panel then
		panel:Hide()
		panel.input:ClearFocus()
	end
end

function U.toggle()
	if not panel then
		return
	end
	if panel:IsShown() then
		U.hide()
	else
		U.show()
	end
end

function U.is_shown()
	return panel and panel:IsShown()
end

return U
