-- Mod parser oracle: writes modLib.parseMod's result for every mod line
-- upstream knows as JSON lines, one object per line, sorted:
--   {"line": <line>, "mods": <mod list or false>, "extra": <string or false>}
-- where false stands for nil, or {"line": <line>, "error": true} when upstream
-- raises an error on the line.
--
-- Lines come from upstream's ModCache, everything upstream parses while
-- starting (uniques, rares, the latest tree), every tree version, every item
-- mod, enchantment and crafting table in data, and the given builds. Every
-- line is parsed live, bypassing the ModCache.
--
-- Run from upstream's src/ directory, usually through oracle/run.py:
--   luajit <repo>/oracle/modparser.lua <out file> <synthetic lines> <id>=<build file>...

local outFile = assert(arg[1], "usage: modparser.lua <out file> <synthetic lines> <id>=<build file>...")
local syntheticFile = assert(arg[2], "usage: modparser.lua <out file> <synthetic lines> <id>=<build file>...")
local oracleDir = arg[0]:match("^(.*)/[^/]+$") or "."
local headless = dofile(oracleDir .. "/headless.lua")
local toJSON = dofile(oracleDir .. "/json.lua")
local builds = headless.parseBuildArgs({ select(3, unpack(arg)) })

headless.start()

-- modLib.parseMod memoises every line in its cache, including everything
-- parsed during startup
local parseMod
local lines = { }
for line in pairs(modLib.parseModCache) do
	lines[line] = true
end
local function add(line)
	for part in line:gmatch("[^\n]+") do
		lines[part] = true
	end
end

-- Every tree version, as builds on old trees parse their nodes
for _, treeVersion in ipairs(treeVersionList) do
	main:LoadTree(treeVersion)
end
for line in pairs(modLib.parseModCache) do
	lines[line] = true
end

-- Strings in the array part of mod entries: { "line 1", "line 2", type = ... }
local function addModTable(t, seen)
	seen = seen or { }
	if seen[t] then
		return
	end
	seen[t] = true
	for _, v in ipairs(t) do
		if type(v) == "string" then
			add(v)
		end
	end
	for k, v in pairs(t) do
		if type(v) == "table" and k ~= "modTags" and k ~= "types" and k ~= "weightKey" and k ~= "weightVal" then
			addModTable(v, seen)
		end
	end
end
for _, modTable in pairs(data.itemMods) do
	addModTable(modTable)
end
addModTable(data.masterMods)
addModTable(data.enchantments)
addModTable(data.veiledMods)
addModTable(data.necropolisMods)
addModTable(data.crucible)
addModTable(data.mapMods)

for _, b in ipairs(builds) do
	headless.loadBuild(b.id, b.path)
end
for line in pairs(modLib.parseModCache) do
	lines[line] = true
end

for line in headless.readFile(syntheticFile):gmatch("[^\n]+") do
	if not line:match("^#") then
		lines[line] = true
	end
end

local sorted = { }
for line in pairs(lines) do
	sorted[#sorted + 1] = line
end
table.sort(sorted)

-- Jewel functions are closures; only their presence is recorded here, what
-- they do is tested through whole builds
local function funcValue()
	return { ["$func"] = true }
end

-- A fresh ModParser: its memo cache is not preloaded from Data/ModCache.lua,
-- so every line is parsed live. Being fresh also matters because parseMod
-- modifies some of its tables while parsing (the DOUBLED form writes into a
-- modNameList entry), so results can depend on earlier lines; parsing in
-- sorted order from a fresh state makes them reproducible.
parseMod = LoadModule("Modules/ModParser", launch)

local f = assert(io.open(outFile, "wb"))
for _, line in ipairs(sorted) do
	local ok, mods, extra = pcall(parseMod, line)
	if ok then
		f:write(toJSON({ line = line, mods = mods or false, extra = extra or false }, funcValue), "\n")
	else
		f:write(toJSON({ line = line, error = true }), "\n")
	end
end
f:close()
print(string.format("%d lines", #sorted))
