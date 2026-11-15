-- Upstream's Lua values for the equivalence tests that compare typed data
-- structures with upstream's (items, passive trees, the data table), and
-- the encoding they come in. Loaded by session.lua, which answers
-- `{"cmd":"oracleValue","fn":<name>,"params":<object>}` with
-- `{"ok":<value>}` or `{"error":..}`; `equivalence::lua_json` reads the
-- value back into a Lua value.
--
-- Encoding, unlike json.lua's keeping everything a Lua value has:
-- - a table is {"#": <id>, "kv": [k1, v1, k2, v2, ...]}, its entries with
--   number keys first in ascending order, then string keys in byte order,
--   so the order is the same in every run (`pairs` order is not: LuaJIT
--   seeds its string hash per process); a table met again (shared or a
--   cycle) is {"$r": <id>}; metatables are left out,
-- - finite numbers are written with %.17g, others as json.lua's {"$num": ..},
-- - a string that is not UTF-8 is {"$bytes": <hex>},
-- - a function is {"$f": "<file>:<line>"}, where it is defined.

local values = { }

local function utf8Valid(s)
	local i, n = 1, #s
	while i <= n do
		local c = s:byte(i)
		local len
		if c < 0x80 then
			len = 1
		elseif c >= 0xC2 and c <= 0xDF then
			len = 2
		elseif c >= 0xE0 and c <= 0xEF then
			len = 3
		elseif c >= 0xF0 and c <= 0xF4 then
			len = 4
		else
			return false
		end
		for j = i + 1, i + len - 1 do
			local d = s:byte(j)
			if not d or d < 0x80 or d > 0xBF then
				return false
			end
		end
		i = i + len
	end
	return true
end

local function encodeString(s)
	if s:find("[\128-\255]") and not utf8Valid(s) then
		return '{"$bytes":"' .. s:gsub(".", function(c) return string.format("%02x", c:byte()) end) .. '"}'
	end
	return '"' .. s:gsub('[%c"\\]', function(c)
		if c == '"' then return '\\"'
		elseif c == "\\" then return "\\\\"
		elseif c == "\n" then return "\\n"
		elseif c == "\t" then return "\\t"
		else return string.format("\\u%04x", c:byte()) end
	end) .. '"'
end

local function encodeNumber(n)
	if n ~= n then
		return '{"$num":"nan"}'
	elseif n == math.huge then
		return '{"$num":"inf"}'
	elseif n == -math.huge then
		return '{"$num":"-inf"}'
	end
	return string.format("%.17g", n)
end

local function keyLess(a, b)
	local ta, tb = type(a), type(b)
	if ta ~= tb then
		-- numbers, strings, booleans, then tables
		local rank = { number = 1, string = 2, boolean = 3, table = 4 }
		return rank[ta] < rank[tb]
	end
	if ta == "boolean" then
		return (not a) and b
	elseif ta == "table" then
		-- tables as keys are game objects (data.gemForSkill's granted
		-- effects): by their id
		return tostring(rawget(a, "id")) < tostring(rawget(b, "id"))
	end
	return a < b
end

-- Encodes `v`; tables in `skip` are written as {"$skip": <name>}
function values.encode(v, skip)
	local out = { }
	local ids = { }
	local nextId = 0
	local encode
	local function encodeTable(t, path)
		if skip and skip[t] then
			out[#out + 1] = '{"$skip":' .. encodeString(skip[t]) .. '}'
			return
		end
		if ids[t] then
			out[#out + 1] = '{"$r":' .. ids[t] .. '}'
			return
		end
		nextId = nextId + 1
		ids[t] = nextId
		local keys = { }
		for k in next, t do
			local kt = type(k)
			if kt ~= "string" and kt ~= "number" and kt ~= "boolean" and kt ~= "table" then
				error("key of type " .. kt .. " at " .. path)
			end
			keys[#keys + 1] = k
		end
		table.sort(keys, keyLess)
		out[#out + 1] = '{"#":' .. nextId .. ',"kv":['
		for i, k in ipairs(keys) do
			if i > 1 then out[#out + 1] = "," end
			encode(k, path)
			out[#out + 1] = ","
			encode(rawget(t, k), path .. "." .. tostring(k))
		end
		out[#out + 1] = "]}"
	end
	function encode(x, path)
		local xt = type(x)
		if xt == "table" then
			encodeTable(x, path)
		elseif xt == "number" then
			out[#out + 1] = encodeNumber(x)
		elseif xt == "string" then
			out[#out + 1] = encodeString(x)
		elseif xt == "boolean" then
			out[#out + 1] = tostring(x)
		elseif xt == "function" then
			local info = debug.getinfo(x, "S")
			out[#out + 1] = encodeString(info.source:gsub("^@", "") .. ":" .. info.linedefined)
			out[#out] = '{"$f":' .. out[#out] .. '}'
		else
			error("value of type " .. xt .. " at " .. path)
		end
	end
	encode(v, "$")
	return table.concat(out)
end

local function copyWithout(t, without)
	local c = { }
	for k, v in pairs(t) do
		if not without[k] then
			c[k] = v
		end
	end
	return c
end

-- What an item table holds besides the item: the data it was made from
local itemRefs = { base = true, affixes = true, enchantments = true, corruptable = true, Object = true, _parentInit = true }

local function jewelRadius()
	local labels = { }
	for i, r in ipairs(data.jewelRadius) do
		labels[i] = r.label
	end
	return labels
end

-- `new("Item", raw, rarity, true)` for each of `args.items` ({raw, rarity}),
-- as upstream's unique and rare databases parse their templates; with
-- `args.templates`, every unique and rare template of the data instead
function values.items(args)
	local list = args.items
	if args.templates then
		list = { }
		local types = { }
		for ty in pairs(data.uniques) do
			types[#types + 1] = ty
		end
		table.sort(types)
		for _, ty in ipairs(types) do
			for _, raw in ipairs(data.uniques[ty]) do
				list[#list + 1] = { kind = "unique " .. ty, raw = raw, rarity = "UNIQUE" }
			end
		end
		for _, raw in ipairs(data.rares) do
			list[#list + 1] = { kind = "rare", raw = raw, rarity = "RARE" }
		end
	end
	local items = { }
	for i, e in ipairs(list) do
		local ok, item = pcall(function()
			return new("Item"):Item(e.raw, e.rarity, true)
		end)
		items[i] = { kind = e.kind, raw = e.raw, rarity = e.rarity }
		if ok then
			items[i].item = copyWithout(item, itemRefs)
		else
			items[i].error = tostring(item)
		end
	end
	return { items = items, jewelRadius = jewelRadius() }
end

-- Loads a build from XML and answers its items by id
function values.buildItems(args)
	loadBuildFromXML(args.xml, args.id)
	local items = { }
	for id, item in pairs(build.itemsTab.items) do
		items[id] = copyWithout(item, itemRefs)
	end
	return { items = items, jewelRadius = jewelRadius() }
end

-- A ModList's mods (its array part), without the class
local function modArray(list)
	if not list then
		return nil
	end
	local out = { }
	for i = 1, #list do
		out[i] = list[i]
	end
	return out
end

-- The passive tree of `args.version` as `main:LoadTree` makes it, in the
-- shape crates/equivalence/tests/tree.rs reads: node fields, links by id,
-- the radius lists as sets of node ids, the lookup tables' node ids
function values.tree(args)
	local tree = main:LoadTree(args.version)
	local function idSet(nodes)
		local s = { }
		for id in pairs(nodes) do
			s[id] = true
		end
		return s
	end
	local nodes = { }
	for id, node in pairs(tree.nodes) do
		if type(id) == "number" then
			local radius
			if node.nodesInRadius then
				radius = { }
				for i, r in pairs(node.nodesInRadius) do
					radius[i] = idSet(r)
				end
			end
			local mods = { }
			for i, m in ipairs(node.mods or { }) do
				mods[i] = { list = m.list, extra = m.extra }
			end
			nodes[id] = {
				dn = node.dn, sd = node.sd, type = node.type,
				ascendancyName = node.ascendancyName, linkedId = node.linkedId,
				x = node.x, y = node.y, angle = node.angle, icon = node.icon,
				unknown = node.unknown, extra = node.extra, mods = mods,
				modList = modArray(node.modList), keystoneMod = node.keystoneMod,
				nodesInRadius = radius,
			}
		end
	end
	local function idMap(m)
		local out = { }
		for name, node in pairs(m) do
			out[name] = { id = node.id }
		end
		return out
	end
	local connectors = { }
	for i, c in ipairs(tree.connectors) do
		connectors[i] = { nodeId1 = c.nodeId1, nodeId2 = c.nodeId2 }
	end
	local classes = { }
	for i, c in pairs(tree.classes) do
		classes[i] = { startNodeId = c.startNodeId }
	end
	local mastery = { }
	for id, e in pairs(tree.masteryEffects or { }) do
		mastery[id] = { modList = modArray(e.modList) }
	end
	return {
		latestTreeVersion = latestTreeVersion,
		nodes = nodes,
		keystoneMap = idMap(tree.keystoneMap),
		notableMap = idMap(tree.notableMap),
		ascendancyMap = idMap(tree.ascendancyMap),
		clusterNodeMap = idMap(tree.clusterNodeMap),
		connectors = connectors,
		orbitRadii = tree.orbitRadii,
		min_x = tree.min_x, min_y = tree.min_y, max_x = tree.max_x, max_y = tree.max_y,
		sockets = idSet(tree.sockets),
		classNameMap = tree.classNameMap,
		classes = classes,
		classNotables = tree.classNotables,
		masteryEffects = mastery,
	}
end

-- The data table of the started program with the flags its skills have
-- after their first calculation (looking up each stat the skill names in
-- its stat map), the copied stat maps left out; plus the globals the typed
-- tables' `from_lua` read and the orders of `pairsOrder`
function values.data()
	for _, ge in pairs(data.skills) do
		local own = rawget(ge, "statMap")
		if type(own) == "table" then
			local names = { }
			for _, s in ipairs(ge.stats or { }) do
				names[#names + 1] = s
			end
			for _, key in ipairs({ "constantStats", "qualityStats" }) do
				for _, e in ipairs(ge[key] or { }) do
					names[#names + 1] = e[1]
				end
			end
			for _, name in ipairs(names) do
				if rawget(own, name) == nil then
					local _ = own[name]
					rawset(own, name, nil)
				end
			end
		end
	end
	-- What the typed data keeps in `pairs` order over number keys, which
	-- LuaJIT gives the same in every run (the encoding sorts them)
	local firstLevels = { }
	for id, ge in pairs(data.skills) do
		firstLevels[id] = next(ge.levels or { })
	end
	local pantheonLines = { }
	for god, g in pairs(data.pantheons) do
		local souls = { }
		for _, soul in pairs(g.souls) do
			local lines = { }
			for _, m in pairs(soul.mods) do
				lines[#lines + 1] = m.line
			end
			souls[#souls + 1] = lines
		end
		pantheonLines[god] = souls
	end
	local flavourIds = { }
	for _, e in pairs(data.flavourText) do
		if e.name and e.id and e.text then
			local list = flavourIds[e.name] or { }
			flavourIds[e.name] = list
			local seen = false
			for _, id in ipairs(list) do
				seen = seen or id == e.id
			end
			if not seen then
				list[#list + 1] = e.id
			end
		end
	end
	return {
		pairsOrder = { firstLevels = firstLevels, pantheonLines = pantheonLines, flavourIds = flavourIds },
		data = data,
		colorCodes = colorCodes,
		SkillType = SkillType,
		ModFlag = ModFlag,
		KeywordFlag = KeywordFlag,
		latestTreeVersion = latestTreeVersion,
		configOptions = LoadModule("Modules/ConfigOptions"),
	}
end

return values
