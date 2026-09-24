--[[
OCWow :: UI

A small, self-contained panel: transcript, input box, and a handful of buttons.
Deliberately plain so it renders on any client build.
]]

local _, ns = ...
local OCWow = ns or OCWow

local U = {}
OCWow.UI = U

local panel
local status
local context_button
local on_send

local TIER_LABEL = {
	[0] = "context: light",
	[1] = "context: normal",
	[2] = "context: full",
}

local function button(parent, text, width, x, y, onClick)
	local b = CreateFrame("Button", nil, parent, "UIPanelButtonTemplate")
	b:SetSize(width, 22)
	b:SetPoint("BOTTOMLEFT", parent, "BOTTOMLEFT", x, y)
	b:SetText(text)
	b:SetScript("OnClick", onClick)
	return b
end

--- Create the panel frame, preferring the standard dialog template and
--- falling back to a backdrop-enabled frame if that template is missing.
local function create_panel_frame()
	local ok, frame = pcall(CreateFrame, "Frame", "OCWowPanel", UIParent, "BasicFrameTemplateWithInset")
	if ok and frame then
		return frame, true
	end

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
		return backdrop, false
	end

	return CreateFrame("Frame", "OCWowPanel", UIParent), false
end

--- Create the panel. `sender` is called with the input text on Send/Enter.
function U.create(sender)
	on_send = sender

	local frame, has_title = create_panel_frame()
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

	if has_title and panel.TitleText then
		panel.TitleText:SetText("OCWow")
	end

	local title = panel:CreateFontString(nil, "OVERLAY", "GameFontNormal")
	title:SetPoint("TOP", panel, "TOP", 0, has_title and -6 or -12)
	title:SetText("OCWow - OpenCode")

	local output = CreateFrame("ScrollingMessageFrame", nil, panel)
	output:SetPoint("TOPLEFT", panel, "TOPLEFT", 16, -30)
	output:SetPoint("BOTTOMRIGHT", panel, "BOTTOMRIGHT", -16, 96)
	output:SetFontObject(ChatFontNormal)
	output:SetFading(false)
	output:SetMaxLines(800)
	output:SetJustifyH("LEFT")
	output:SetHyperlinksEnabled(false)
	output:EnableMouseWheel(true)
	output:SetScript("OnMouseWheel", function(self, delta)
		if delta > 0 then
			self:ScrollUp()
		else
			self:ScrollDown()
		end
	end)
	panel.output = output

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

	button(panel, "New", 60, 132, 34, function()
		if on_send then
			on_send("/new")
		end
	end)

	button(panel, "Stop", 60, 196, 34, function()
		if on_send then
			on_send("/stop")
		end
	end)

	button(panel, "Help", 60, 260, 34, function()
		if on_send then
			on_send("/help")
		end
	end)

	button(panel, "Hide", 60, 324, 34, function()
		U.hide()
	end)

	status = panel:CreateFontString(nil, "OVERLAY", "GameFontHighlightSmall")
	status:SetPoint("BOTTOMLEFT", panel, "BOTTOMLEFT", 16, 12)
	status:SetPoint("BOTTOMRIGHT", panel, "BOTTOMRIGHT", -16, 12)
	status:SetJustifyH("LEFT")
	status:SetText("ready")

	return panel
end

--- Append a message to the transcript.
function U.add(text, r, g, b)
	if not panel then
		return
	end
	panel.output:AddMessage(text or "", r or 1, g or 1, b or 1)
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
