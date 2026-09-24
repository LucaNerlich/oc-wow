--[[
OCWow :: Context

Builds a compact, tiered snapshot of the player's game state so the model knows
what is happening without drowning the prompt.

  * tier 0 (always): where, who, group, combat, difficulty
  * tier 1 (on request): target, vitals, gear, gold, quests
  * tier 2 (explicit): guild, professions, playtime

Every accessor is wrapped so a missing API on one client build cannot break the
snapshot.
]]

local _, ns = ...
local OCWow = ns or OCWow

local C = {}
OCWow.Context = C

C.TIER_LIGHT = 0
C.TIER_NORMAL = 1
C.TIER_FULL = 2

-- Some client builds return "secret" values from combat APIs. They cannot be
-- formatted, concatenated or compared, so drop them at the source. The checker
-- is looked up dynamically because it does not exist on older clients.
local secret_check = rawget(_G, "issecretvalue") or rawget(_G, "isSecretValue")

local function is_secret(value)
	if value == nil or secret_check == nil then
		return false
	end
	local ok, secret = pcall(secret_check, value)
	return ok and secret and true or false
end

local function call(fn, ...)
	-- Six values covers every API used here (GetInstanceInfo's difficulty name
	-- is the fourth, for example).
	local ok, a, b, c, d, e, f = pcall(fn, ...)
	if not ok then
		return
	end
	if is_secret(a) then
		a = nil
	end
	if is_secret(b) then
		b = nil
	end
	if is_secret(c) then
		c = nil
	end
	if is_secret(d) then
		d = nil
	end
	if is_secret(e) then
		e = nil
	end
	if is_secret(f) then
		f = nil
	end
	return a, b, c, d, e, f
end

local function round(n)
	if n == nil then
		return nil
	end
	return math.floor(n + 0.5)
end

local function coords()
	local x, y = call(GetPlayerMapPosition, "player")
	if x and y and (x ~= 0 or y ~= 0) then
		return round(x * 100), round(y * 100)
	end
end

local function player_line()
	local name = call(UnitName, "player") or "?"
	local _, class = call(UnitClass, "player")
	local level = call(UnitLevel, "player")
	local spec
	local specIndex = call(GetSpecialization)
	if specIndex then
		local specName = call(GetSpecializationInfo, specIndex)
		spec = specName
	end
	local parts = { name }
	if level then
		parts[#parts + 1] = "level " .. level
	end
	if class then
		parts[#parts + 1] = class
	end
	if spec then
		parts[#parts + 1] = spec
	end
	return table.concat(parts, ", ")
end

local function zone_line()
	local zone = call(GetZoneText) or "unknown"
	local subzone = call(GetSubZoneText)
	local x, y = coords()
	local parts = { zone }
	if subzone and subzone ~= "" then
		parts[#parts + 1] = subzone
	end
	if x then
		parts[#parts + 1] = string.format("(%d,%d)", x, y)
	end
	return table.concat(parts, " / ")
end

local function group_line()
	local count = call(GetNumGroupMembers) or 0
	if count == 0 then
		return "solo"
	end
	local kind = call(IsInRaid) and "raid" or "party"
	return string.format("%s of %d", kind, count)
end

local function difficulty_line()
	local _, instanceType, _, difficultyName = call(GetInstanceInfo)
	if not instanceType or instanceType == "none" then
		return nil
	end
	return string.format("%s%s", instanceType, difficultyName and (" (" .. difficultyName .. ")") or "")
end

local function target_line(unit)
	if not call(UnitExists, unit) then
		return nil
	end
	local name = call(UnitName, unit) or "?"
	local level = call(UnitLevel, unit)
	local health = call(UnitHealth, unit)
	local healthMax = call(UnitHealthMax, unit)
	local reaction = call(UnitReaction, unit, "player")
	local parts = { name }
	if level and level > 0 then
		parts[#parts + 1] = "level " .. level
	end
	if health and healthMax and healthMax > 0 then
		parts[#parts + 1] = string.format("%d%% hp", round(health / healthMax * 100))
	end
	if reaction then
		local labels = { "hated", "hostile", "unfriendly", "neutral", "friendly", "honored" }
		parts[#parts + 1] = labels[reaction] or "neutral"
	end
	return table.concat(parts, ", ")
end

local function vitals_line()
	local health = call(UnitHealth, "player")
	local healthMax = call(UnitHealthMax, "player")
	local power = call(UnitPower, "player")
	local powerMax = call(UnitPowerMax, "player")
	local parts = {}
	if health and healthMax and healthMax > 0 then
		parts[#parts + 1] = string.format("hp %d/%d", health, healthMax)
	end
	if power and powerMax and powerMax > 0 then
		parts[#parts + 1] = string.format("power %d/%d", power, powerMax)
	end
	if #parts == 0 then
		return nil
	end
	return table.concat(parts, ", ")
end

local function gear_line()
	local _, equipped = call(GetAverageItemLevel)
	if equipped and equipped > 0 then
		return string.format("avg ilvl %.1f", equipped)
	end
end

local function money_line()
	local money = call(GetMoney)
	if not money then
		return nil
	end
	local gold = math.floor(money / 10000)
	local silver = math.floor((money % 10000) / 100)
	return string.format("%dg %ds", gold, silver)
end

local function durability_line()
	local total, worst = 0, 100
	local slots = 0
	for slot = 1, 19 do
		local current, maximum = call(GetInventoryItemDurability, slot)
		if current and maximum and maximum > 0 then
			slots = slots + 1
			local pct = current / maximum * 100
			total = total + pct
			if pct < worst then
				worst = pct
			end
		end
	end
	if slots == 0 then
		return nil
	end
	return string.format("avg %d%%, lowest %d%%", round(total / slots), round(worst))
end

local function bags_line()
	local free = 0
	for bag = 0, 4 do
		local slots = call(GetContainerNumFreeSlots, bag)
		free = free + (slots or 0)
	end
	return free .. " free slots"
end

local function quests_line(limit)
	local count = call(GetNumQuestLogEntries) or 0
	if count == 0 then
		return nil
	end
	local titles = {}
	for i = 1, math.min(count, limit or 3) do
		local title = call(GetQuestLogTitle, i)
		if title and title ~= "" then
			titles[#titles + 1] = title
		end
	end
	local line = string.format("%d in log", count)
	if #titles > 0 then
		line = line .. ": " .. table.concat(titles, "; ")
	end
	return line
end

local function guild_line()
	local name = call(GetGuildInfo, "player")
	if name and name ~= "" then
		return name
	end
end

local function professions_line()
	local first, second = call(GetProfessions)
	local names = {}
	for _, index in ipairs({ first, second }) do
		if index then
			local name, _, skill = call(GetProfessionInfo, index)
			if name then
				names[#names + 1] = string.format("%s %d", name, skill or 0)
			end
		end
	end
	if #names > 0 then
		return table.concat(names, ", ")
	end
end

local function add(lines, key, value)
	if value == nil or value == "" then
		return
	end
	-- Guarded: a value derived from a secret would make the concatenation fail.
	local ok, line = pcall(function()
		return key .. ": " .. tostring(value)
	end)
	if ok and type(line) == "string" then
		lines[#lines + 1] = line
	end
end

--- Build the game-state block for the given tier.
local function build_snapshot(tier)
	tier = tier or C.TIER_LIGHT
	local lines = {}
	add(lines, "player", player_line())
	add(lines, "location", zone_line())
	add(lines, "group", group_line())
	add(lines, "in_combat", call(UnitAffectingCombat, "player") and "yes" or "no")
	add(lines, "resting", call(IsResting) and "yes" or "no")
	add(lines, "instance", difficulty_line())

	if tier >= C.TIER_NORMAL then
		add(lines, "target", target_line("target"))
		add(lines, "focus", target_line("focus"))
		add(lines, "vitals", vitals_line())
		add(lines, "gear", gear_line())
		add(lines, "money", money_line())
		add(lines, "durability", durability_line())
		add(lines, "bags", bags_line())
		add(lines, "quests", quests_line(3))
	end

	if tier >= C.TIER_FULL then
		add(lines, "guild", guild_line())
		add(lines, "professions", professions_line())
		local time = call(GetTime)
		if time then
			add(lines, "played_seconds", tostring(math.floor(time)))
		end
	end

	return table.concat(lines, "\n")
end

--- Build the game-state block for the given tier.
---
--- Wrapped in pcall: a context failure must never block a prompt. Returns an
--- empty string when anything goes wrong.
function C.snapshot(tier)
	local ok, result = pcall(build_snapshot, tier)
	if ok and type(result) == "string" then
		return result
	end
	return ""
end
