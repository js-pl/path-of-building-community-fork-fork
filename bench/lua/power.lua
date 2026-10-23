-- Node power and anoint power as calc jobs.
--
-- Node power candidates are not reimplemented: a plan pass runs upstream's
-- CalcsTabClass:PowerBuilder with a stub calculator that only records the
-- overrides it would calculate. Anoint power mirrors
-- NotableDBClass:ListBuilder, which is tied to UI controls.
--
-- In sync with upstream v2.67.2 (src/Classes/CalcsTab.lua,
-- src/Classes/NotableDBControl.lua).

local pob = require("pob")

local t_insert = table.insert
local t_concat = table.concat
local t_sort = table.sort
local s_format = string.format

local power = { }

-- Output fields that make up a calc result: every power stat plus the fields
-- CalculateCombinedOffDefStat reads.
local combinedFields = {
	"LifeUnreserved", "Life", "Armour", "EnergyShieldRecoveryCap", "EnergyShield", "Evasion",
	"LifeRegenRecovery", "EnergyShieldRegenRecovery", "CombinedDPS",
}

local function formatValue(value)
	if type(value) == "number" then
		return s_format("%.17g", value)
	end
	return "<" .. type(value) .. ">"
end

-- Serialises the parts of a calc output that power calculations use.
function power.fingerprint(output)
	local values = { }
	for _, stat in ipairs(data.powerStatList) do
		if stat.stat then
			t_insert(values, formatValue(data.powerStatList.GetFromOutput(output, stat, true)))
		end
	end
	for _, field in ipairs(combinedFields) do
		t_insert(values, formatValue(output[field]))
	end
	t_insert(values, formatValue(output.Minion and output.Minion.CombinedDPS))
	return t_concat(values, "\t")
end

local function escape(text)
	return (text:gsub("\\", "\\\\"):gsub("\t", "\\t"):gsub("\n", "\\n"))
end

local function nodeSetKey(kind, nodes)
	local list = { }
	for node in pairs(nodes) do
		t_insert(list, node)
	end
	if #list == 1 then
		-- PowerBuilder caches single node calcs by modKey
		return kind .. ":" .. escape(list[1].modKey)
	end
	local parts = { }
	for _, node in ipairs(list) do
		t_insert(parts, node.id .. "/" .. escape(node.modKey))
	end
	t_sort(parts)
	return kind .. "*:" .. t_concat(parts, ",")
end

-- Stable key for a misc calculator override used by PowerBuilder
function power.overrideKey(override)
	if override.addNodes then
		return nodeSetKey("add", override.addNodes)
	elseif override.removeNodes then
		return nodeSetKey("remove", override.removeNodes)
	end
	error("unsupported override")
end

-- Selects the power stat like the tree tab does; "combined" is the
-- default "Offence/Defence" selection.
function power.setPowerStat(calcsTab, statName)
	if statName == "combined" then
		calcsTab.powerStat = data.powerStatList[1]
		return
	end
	for _, stat in ipairs(data.powerStatList) do
		if stat.stat == statName then
			calcsTab.powerStat = stat
			return
		end
	end
	error("unknown power stat: " .. statName)
end

-- Runs fn with the misc calculator replaced by wrap(calcFunc)
local function withCalcFunc(calcsTab, wrap, fn)
	local orig = calcsTab.miscCalculator[1]
	calcsTab.miscCalculator[1] = wrap(orig)
	local ok, err = pcall(fn)
	calcsTab.miscCalculator[1] = orig
	if not ok then
		error(err, 0)
	end
end

-- Gives repeated keys a running suffix so every call stays a separate job
local function uniqueKey(seen, key)
	local n = (seen[key] or 0) + 1
	seen[key] = n
	return n == 1 and key or key .. "#" .. n
end

-- Lists the calcs upstream PowerBuilder performs, without calculating them.
-- Returns jobs sorted by key.
function power.planNodePower(calcsTab)
	local jobs = { }
	local seen = { }
	local calcBase = calcsTab.miscCalculator[2]
	withCalcFunc(calcsTab, function(calcFunc)
		return function(override, useFullDPS)
			local key = uniqueKey(seen, power.overrideKey(override))
			t_insert(jobs, {
				key = key,
				run = function()
					return calcFunc(override, useFullDPS)
				end,
			})
			return calcBase
		end
	end, function()
		calcsTab:PowerBuilder()
	end)
	t_sort(jobs, function(a, b) return a.key < b.key end)
	return jobs
end

-- Runs upstream PowerBuilder unchanged and records every calc it performs.
-- Returns the results by key and the total PowerBuilder time.
function power.runUpstreamNodePower(calcsTab)
	local results = { }
	local seen = { }
	local t0
	withCalcFunc(calcsTab, function(calcFunc)
		return function(override, useFullDPS)
			local start = pob.clock()
			local output = calcFunc(override, useFullDPS)
			local ms = pob.clock() - start
			t_insert(results, { key = uniqueKey(seen, power.overrideKey(override)), ms = ms, fingerprint = power.fingerprint(output) })
			return output
		end
	end, function()
		t0 = pob.clock()
		calcsTab:PowerBuilder()
		t0 = pob.clock() - t0
	end)
	return results, t0
end

-- Equipped item that anoints are tested on, as NotableDBControl uses the
-- displayed item
local function anointTarget(build)
	local slot = build.itemsTab.slots["Amulet"]
	local item = slot and build.itemsTab.items[slot.selItemId]
	return item
end

-- Jobs for anoint power: one calc per anointable notable plus the base calc
-- with the anoint removed. Returns nil if there is no amulet.
function power.planAnointPower(build)
	local itemsTab = build.itemsTab
	local item = anointTarget(build)
	if not item then
		return nil
	end
	itemsTab.displayItem = item
	itemsTab.anointEnchantSlot = 1
	local itemType = item.base.type
	local calcFunc = build.calcsTab:GetMiscCalculator()
	local jobs = { }
	local seen = { }
	for _, node in pairs(build.spec.tree.nodes) do
		-- NotableDBControl IsAnointableNode, without search filters
		if node.recipe and #node.recipe >= 1 and node.modKey ~= "" then
			t_insert(jobs, {
				key = uniqueKey(seen, "anoint:" .. escape(node.dn)),
				run = function()
					return calcFunc({ repSlotName = itemType, repItem = itemsTab:anointItem(node) })
				end,
			})
		end
	end
	t_sort(jobs, function(a, b) return a.key < b.key end)
	-- NotableDBControl runs the base calc before any candidate
	t_insert(jobs, 1, {
		key = "anoint-base",
		run = function()
			return calcFunc({ repSlotName = itemType, repItem = itemsTab:anointItem(nil) })
		end,
	})
	return jobs
end

return power
