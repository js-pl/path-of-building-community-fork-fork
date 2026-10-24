-- Data oracle: loads every data file Modules/Data.lua loads, with the same
-- arguments, and writes one JSON line per table with the FNV-1a hash of its
-- canonical JSON (functions as {"$func": "<file>:<line>"}):
--   {"bytes": <length>, "fnv": "<16 hex digits>", "key": "data.skills"}
-- With a second argument, also writes each table's JSON to that directory.
--
-- Run from upstream's src/ directory, usually through oracle/run.py:
--   luajit <repo>/oracle/data.lua <out file> [<json dir>]

local outFile = assert(arg[1], "usage: data.lua <out file> [<json dir>]")
local jsonDir = arg[2]
local oracleDir = arg[0]:match("^(.*)/[^/]+$") or "."
local headless = dofile(oracleDir .. "/headless.lua")
local toJSON = dofile(oracleDir .. "/json.lua")

headless.start()

local bit = require("bit")

-- FNV-1a 64 over a string, with two 32 bit halves since LuaJIT's bit
-- operations are 32 bit
local function fnv1a(s)
	-- offset basis 0xcbf29ce484222325
	local hi, lo = 0xcbf29ce4, 0x84222325
	for i = 1, #s do
		lo = bit.bxor(lo, s:byte(i))
		-- multiply by the prime 0x00000100000001b3:
		-- (hi * 2^32 + lo) * (2^40 + 0x1b3), modulo 2^64
		local lo_u = lo % 4294967296
		local hi_u = hi % 4294967296
		local p_lo = lo_u * 0x1b3
		local carry = math.floor(p_lo / 4294967296)
		local new_lo = p_lo % 4294967296
		local new_hi = (hi_u * 0x1b3 + carry + (lo_u * 256) % 4294967296) % 4294967296
		hi, lo = new_hi, new_lo
	end
	return string.format("%08x%08x", hi % 4294967296, lo % 4294967296)
end

local function funcValue(f)
	local info = debug.getinfo(f, "S")
	return { ["$func"] = info.source:gsub("^@", "") .. ":" .. info.linedefined }
end

-- Same as makeSkillMod, makeFlagMod and makeSkillDataMod in Data.lua
local function makeSkillMod(modName, modType, modVal, flags, keywordFlags, ...)
	return { name = modName, type = modType, value = modVal, flags = flags or 0, keywordFlags = keywordFlags or 0, ... }
end
local function makeFlagMod(modName, ...)
	return makeSkillMod(modName, "FLAG", true, 0, 0, ...)
end
local function makeSkillDataMod(dataKey, dataValue, ...)
	return makeSkillMod("SkillData", "LIST", { key = dataKey, value = dataValue }, 0, 0, ...)
end

-- The tables, in the order crates/importer/src/lib.rs builds them
local out = { }
local function add(key, value)
	out[#out + 1] = { key = key, value = value }
end
local function load(name, ...)
	return LoadModule("Data/" .. name, ...)
end

for _, key in ipairs({ "keystones", "ailmentTypeList", "elementalAilmentTypeList", "nonDamagingAilmentTypeList",
		"nonElementalAilmentTypeList", "nonDamagingAilment" }) do
	add("data." .. key, data[key])
end
local misc = { }
load("Misc", misc)
add("Misc", misc)
add("data.modScalability", load("ModScalability"))
for _, file in ipairs({ "ModExplicit", "ModItemExclusive", "ModCorrupted", "ModDelve", "ModSynthesis", "ModScourge",
		"ModEldritch", "ModFlask", "ModTincture", "ModGraft", "ModJewel", "ModJewelAbyss", "ModJewelCluster",
		"ModJewelCharm", "ModFoulborn", "ModMaster", "EnchantmentHelmet", "EnchantmentBoots", "EnchantmentGloves",
		"EnchantmentBelt", "EnchantmentBody", "EnchantmentWeapon", "EnchantmentFlask", "Essence", "ModVeiled",
		"BeastCraft", "ModNecropolis", "Crucible", "Pantheons", "Costs", "ModMap", "ClusterJewels",
		"TimelessJewelData/LegionTradeIds", "TimelessJewelData/NodeIndexMapping",
		"TimelessJewelData/AbyssNotableNames", "Rares", "FlavourText" }) do
	add(file, load(file))
end
local bosses = { }
load("Bosses", bosses)
add("Bosses", bosses)
local bossSkills, bossSkillsList = load("BossSkills")
add("BossSkills.1", bossSkills)
add("BossSkills.2", bossSkillsList)
add("SkillStatMap", load("SkillStatMap", makeSkillMod, makeFlagMod, makeSkillDataMod))
local skills = { }
for _, t in ipairs({ "act_str", "act_dex", "act_int", "other", "glove", "minion", "spectre", "sup_str", "sup_dex", "sup_int" }) do
	load("Skills/" .. t, skills, makeSkillMod, makeFlagMod, makeSkillDataMod)
end
add("Skills", skills)
add("Gems", load("Gems"))
local minions = { }
load("Minions", minions, makeSkillMod, makeFlagMod)
add("Minions", minions)
local spectres = { }
load("Spectres", spectres, makeSkillMod, makeFlagMod)
add("Spectres", spectres)
local itemTypes = { "axe", "bow", "claw", "dagger", "fishing", "mace", "staff", "sword", "wand", "helmet", "body",
	"gloves", "boots", "shield", "quiver", "amulet", "ring", "belt", "jewel", "flask", "tincture", "graft" }
local bases = { }
for _, t in ipairs(itemTypes) do
	load("Bases/" .. t, bases)
end
add("Bases", bases)
for _, t in ipairs(itemTypes) do
	add("Uniques/" .. t, load("Uniques/" .. t))
end
add("Uniques/Special/race", load("Uniques/Special/race"))
add("Uniques/Special/WatchersEye", load("Uniques/Special/WatchersEye"))
add("ModCache", (function() local c = { } load("ModCache", c) return c end)())
for _, g in ipairs({ "ModFlag", "KeywordFlag", "SkillType" }) do
	add("global." .. g, _G[g])
end

local f = assert(io.open(outFile, "wb"))
for _, entry in ipairs(out) do
	local json = toJSON(entry.value, funcValue)
	f:write(toJSON({ key = entry.key, fnv = fnv1a(json), bytes = #json }), "\n")
	if jsonDir then
		local jf = assert(io.open(jsonDir .. "/" .. entry.key:gsub("/", "_") .. ".json", "wb"))
		jf:write(json, "\n")
		jf:close()
	end
end
f:close()
print(string.format("%d tables", #out))
