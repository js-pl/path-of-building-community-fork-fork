-- Generates corpus builds with upstream's own build API and saves them as PoB
-- XML. Every choice comes from a seeded PRNG and sorted lists, so the same seed
-- gives the same build.
--
-- Run from upstream's src/ directory, usually through `oracle/run.py generate`:
--   luajit <repo>/corpus/generate.lua <out dir> <first seed> <count>

local outDir = assert(arg[1], "usage: generate.lua <out dir> <first seed> <count>")
local firstSeed = assert(tonumber(arg[2]), "first seed must be a number")
local count = assert(tonumber(arg[3]), "count must be a number")
local repoDir = arg[0]:match("^(.*)/corpus/[^/]+$") or "../../.."
local headless = dofile(repoDir .. "/oracle/headless.lua")
headless.start(true)

local bit = require("bit")

-- xorshift32; the state stays a 32 bit integer, so it is exact in LuaJIT
local rng = { }
function rng.seed(seed)
	rng.state = bit.tobit(seed * 2654435761 % 4294967296)
	if rng.state == 0 then
		rng.state = 1
	end
	for _ = 1, 10 do
		rng.next()
	end
end
function rng.next()
	local x = rng.state
	x = bit.bxor(x, bit.lshift(x, 13))
	x = bit.bxor(x, bit.rshift(x, 17))
	x = bit.bxor(x, bit.lshift(x, 5))
	rng.state = x
	return x % 4294967296 / 4294967296
end
-- Integer in [lo, hi]
function rng.int(lo, hi)
	return lo + math.floor(rng.next() * (hi - lo + 1))
end
function rng.chance(p)
	return rng.next() < p
end
function rng.pick(list)
	assert(#list > 0, "pick from empty list")
	return list[rng.int(1, #list)]
end
-- Weighted pick; weight(item) returns a number >= 0
function rng.pickWeighted(list, weight)
	local total = 0
	for _, v in ipairs(list) do
		total = total + weight(v)
	end
	local r = rng.next() * total
	for _, v in ipairs(list) do
		r = r - weight(v)
		if r < 0 then
			return v
		end
	end
	return list[#list]
end

-- Uses next() directly, since pairs() is replaced while saving
local function sortedKeys(t)
	local keys = { }
	for k in next, t do
		keys[#keys + 1] = k
	end
	table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
	return keys
end

-- Upstream saves builds in pairs() order in many places (XML attributes,
-- config entries, item text), which differs between processes. Saving only
-- serialises, so while saving, global pairs() visits keys in sorted order.
local function sortedPairs(t)
	local keys = sortedKeys(t)
	local i = 0
	return function()
		i = i + 1
		local k = keys[i]
		if k ~= nil then
			return k, t[k]
		end
	end
end

-- Some savers cache pairs() as a local, so their unordered children are
-- sorted afterwards: parent element -> child element -> sort key
local unordered = {
	ItemSet = {
		Slot = function(n) return "Slot\0" .. n.attrib.name end,
		SocketIdURL = function(n) return "SocketIdURL\0" .. n.attrib.name end,
	},
	Sockets = {
		Socket = function(n) return string.format("%08d", tonumber(n.attrib.nodeId)) end,
	},
}
local function sortUnordered(node)
	local keyFuncs = unordered[node.elem]
	if keyFuncs then
		local children, positions = { }, { }
		for i, n in ipairs(node) do
			if type(n) == "table" and keyFuncs[n.elem] then
				children[#children + 1] = { key = keyFuncs[n.elem](n), node = n }
				positions[#positions + 1] = i
			end
		end
		table.sort(children, function(a, b) return a.key < b.key end)
		for j, i in ipairs(positions) do
			node[i] = children[j].node
		end
	end
	for _, n in ipairs(node) do
		if type(n) == "table" then
			sortUnordered(n)
		end
	end
end

-- Same as buildMode:SaveDB, with the sections after <Build> sorted by name
local function saveXML()
	local globalPairs = pairs
	pairs = sortedPairs
	local ok, result = pcall(function()
		local root = { elem = "PathOfBuilding" }
		local node = { elem = "Build" }
		build:Save(node)
		root[1] = node
		for _, elem in ipairs(sortedKeys(build.savers)) do
			local saverNode = { elem = elem }
			build.savers[elem]:Save(saverNode)
			root[#root + 1] = saverNode
		end
		sortUnordered(root)
		return assert(common.xml.ComposeXML(root))
	end)
	pairs = globalPairs
	assert(ok, result)
	return result
end

-- Static lists, sorted so the PRNG sees them in a fixed order

local classCombos = { }
do
	local tree = build.spec.tree
	for _, classId in ipairs(sortedKeys(tree.classes)) do
		local class = tree.classes[classId]
		for _, ascId in ipairs(sortedKeys(class.classes)) do
			classCombos[#classCombos + 1] = { classId = classId, ascId = ascId, name = class.name .. "/" .. class.classes[ascId].name }
		end
	end
end
local bloodlineIds = sortedKeys(build.spec.tree.alternate_ascendancies or { })

local activeGems = { }
local supportGems = { }
for _, id in ipairs(sortedKeys(data.gems)) do
	local gem = data.gems[id]
	if not gem.grantedEffect.unsupported then
		if gem.grantedEffect.support then
			supportGems[#supportGems + 1] = gem
		else
			activeGems[#activeGems + 1] = gem
		end
	end
end

-- Main skill categories, chosen in turn so every kind of skill shows up
local categories = { "attack", "spell", "minion", "totem", "trap", "mine", "brand", "channelling", "warcry", "vaal" }
local function gemsWithTag(tag)
	local list = { }
	for _, gem in ipairs(activeGems) do
		if gem.tags[tag] then
			list[#list + 1] = gem
		end
	end
	assert(#list > 0, "no gems tagged " .. tag)
	return list
end
local gemsByCategory = { }
for _, cat in ipairs(categories) do
	gemsByCategory[cat] = gemsWithTag(cat)
end
local auraGems = gemsWithTag("aura")
local curseGems = gemsWithTag("curse")

local function dbItems(db)
	local list = { }
	for _, name in ipairs(sortedKeys(db.list)) do
		list[#list + 1] = db.list[name]
	end
	return list
end
local uniqueItems = dbItems(main.uniqueDB)
local rareItems = dbItems(main.rareDB)

local configOptions = { }
for _, opt in ipairs(LoadModule("Modules/ConfigOptions")) do
	if opt.var and (opt.type == "check" or opt.type == "count" or opt.type == "list") then
		configOptions[#configOptions + 1] = opt
	end
end

-- Build steps

local function allocTree(points, ascPoints)
	local spec = build.spec
	local nodeIds = sortedKeys(spec.nodes)
	-- Only nodes next to the allocated tree are taken, so the path is the node
	-- itself and does not depend on how upstream breaks ties between paths
	local function candidates(ascendancy)
		local list = { }
		for _, id in ipairs(nodeIds) do
			local node = spec.nodes[id]
			if not node.alloc and node.path and #node.path == 1 and node.type ~= "ClassStart"
				and node.type ~= "AscendClassStart" and not node.isProxy and not node.expansionJewel
				and (node.type ~= "Mastery" or node.masteryEffects)
				and (ascendancy and node.ascendancyName == ascendancy or not ascendancy and not node.ascendancyName) then
				list[#list + 1] = node
			end
		end
		return list
	end
	local weights = { Normal = 1, Notable = 4, Keystone = 2, Socket = 2, Mastery = 3 }
	local usedEffects = { }
	local function alloc(node)
		if node.type == "Mastery" then
			local effects = { }
			for _, e in ipairs(node.masteryEffects) do
				if not usedEffects[e.effect] then
					effects[#effects + 1] = e.effect
				end
			end
			if #effects == 0 then
				return false
			end
			local effectId = rng.pick(effects)
			usedEffects[effectId] = true
			spec.masterySelections[node.id] = effectId
			local effect = spec.tree.masteryEffects[effectId]
			node.sd = effect.sd
			node.allMasteryOptions = false
			spec.tree:ProcessStats(node)
		end
		spec:AllocNode(node)
		return true
	end
	for _ = 1, points do
		local list = candidates(nil)
		if #list == 0 then
			break
		end
		alloc(rng.pickWeighted(list, function(n) return weights[n.type] or 1 end))
	end
	local ascName = spec.curAscendClassName
	if ascName and ascName ~= "None" then
		for _ = 1, ascPoints do
			local list = candidates(ascName)
			if #list == 0 then
				break
			end
			alloc(rng.pick(list))
		end
	end
end

local function gemInstance(gem)
	local level = gem.naturalMaxLevel
	if rng.chance(0.3) and gem.grantedEffect.levels[level + 1] then
		level = level + 1
	end
	return {
		gemId = gem.id,
		nameSpec = gem.name,
		level = level,
		quality = rng.pick({ 0, 0, 20, 23 }),
		enabled = true,
		count = 1,
		enableGlobal1 = true,
		enableGlobal2 = true,
	}
end

local function addSocketGroup(active, supportCount, label)
	local group = { label = label, enabled = true, gemList = { gemInstance(active) } }
	for _ = 1, supportCount do
		group.gemList[#group.gemList + 1] = gemInstance(rng.pick(supportGems))
	end
	table.insert(build.skillsTab.socketGroupList, group)
	build.skillsTab:ProcessSocketGroup(group)
	return group
end

local function newItem(dbItem)
	local item = new("Item", dbItem.raw)
	if item.variantList and #item.variantList > 1 then
		item.variant = rng.int(1, #item.variantList)
		if item.hasAltVariant then
			item.variantAlt = rng.int(1, #item.variantList)
		end
		item:BuildAndParseRaw()
	end
	item:NormaliseQuality()
	return item
end

local function equip(slotName, filter)
	local itemsTab = build.itemsTab
	local slot = itemsTab.slots[slotName]
	if not slot or not slot:IsShown() then
		return nil
	end
	local pool = rng.chance(0.65) and uniqueItems or rareItems
	local list = { }
	for _, dbItem in ipairs(pool) do
		if itemsTab:IsItemValidForSlot(dbItem, slotName) and (not filter or filter(dbItem)) then
			list[#list + 1] = dbItem
		end
	end
	if #list == 0 then
		return nil
	end
	local item = newItem(rng.pick(list))
	itemsTab:AddItem(item, true)
	slot:SetSelItemId(item.id)
	itemsTab:PopulateSlots()
	return item
end

local function equipItems(mainGem)
	local weaponTypes = mainGem.grantedEffect.weaponTypes
	equip("Weapon 1", weaponTypes and function(item) return weaponTypes[item.type] end)
	equip("Weapon 2")
	for _, slotName in ipairs({ "Helmet", "Body Armour", "Gloves", "Boots", "Amulet", "Ring 1", "Ring 2", "Belt" }) do
		equip(slotName)
	end
	for i = 1, 5 do
		if equip("Flask " .. i) and rng.chance(0.7) then
			build.itemsTab.slots["Flask " .. i].active = true
		end
	end
	-- Jewels in allocated tree sockets
	for _, id in ipairs(sortedKeys(build.spec.nodes)) do
		local node = build.spec.nodes[id]
		if node.alloc and node.type == "Socket" and rng.chance(0.7) then
			equip("Jewel " .. id)
		end
	end
end

local function setConfig(optionCount)
	local input = build.configTab.input
	for _ = 1, optionCount do
		local opt = rng.pick(configOptions)
		if opt.type == "check" then
			input[opt.var] = true
		elseif opt.type == "count" then
			input[opt.var] = rng.int(1, 10)
		elseif opt.type == "list" then
			input[opt.var] = rng.pick(opt.list).val
		end
	end
	build.configTab:BuildModList()
end

local function generate(seed)
	rng.seed(seed)
	newBuild()
	local combo = classCombos[seed % #classCombos + 1]
	local category = categories[seed % #categories + 1]
	local spec = build.spec
	spec:SelectClass(combo.classId)
	spec:SelectAscendClass(combo.ascId)
	if #bloodlineIds > 0 and rng.chance(0.25) then
		spec:SelectSecondaryAscendClass(rng.pick(bloodlineIds))
	end

	local level = rng.int(80, 100)
	build.characterLevel = level
	build.characterLevelAutoMode = false
	allocTree(level - 1 + 22, 8)

	local mainGem = rng.pick(gemsByCategory[category])
	addSocketGroup(mainGem, rng.int(2, 5), "Main")
	if rng.chance(0.7) then
		addSocketGroup(rng.pick(auraGems), rng.int(0, 2), "Aura")
	end
	if rng.chance(0.5) then
		addSocketGroup(rng.pick(curseGems), rng.int(0, 1), "Curse")
	end
	for i = 1, rng.int(0, 2) do
		addSocketGroup(rng.pick(activeGems), rng.int(0, 3), "Other " .. i)
	end
	build.mainSocketGroup = 1
	-- Full DPS in a third of the builds
	if seed % 3 == 0 then
		for _, group in ipairs(build.skillsTab.socketGroupList) do
			group.includeInFullDPS = rng.chance(0.7)
		end
		build.skillsTab.socketGroupList[1].includeInFullDPS = true
	end

	equipItems(mainGem)
	setConfig(rng.int(3, 15))

	build.buildFlag = true
	runCallback("OnFrame")
	local id = string.format("gen-%04d", seed)
	local xmlText = saveXML()
	local f = assert(io.open(outDir .. "/" .. id .. ".xml", "wb"))
	f:write(xmlText)
	f:close()
	print(string.format("%s %s %s %s", id, combo.name, category, mainGem.name))
end

for seed = firstSeed, firstSeed + count - 1 do
	generate(seed)
end
