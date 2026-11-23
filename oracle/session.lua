-- The engine API on the oracle: crates/session/lua/session.lua, the session
-- the translation runs, on upstream headless on LuaJIT. JSON requests on
-- stdin, one per line; one JSON response per line on stdout, as `pob serve`.
--
-- Run from upstream's src/ directory (upstream uses paths relative to it):
--   luajit <repo>/oracle/session.lua
-- See docs/drop-translation.md: the reference the session tests can use
-- instead of the translation.

local oracleDir = arg[0]:match("^(.*)/[^/]+$") or "."
local headless = dofile(oracleDir .. "/headless.lua")

-- stdout is the protocol: upstream's prints go to stderr
local out = io.stdout
function print(...)
	local parts = { }
	for i = 1, select("#", ...) do
		parts[i] = tostring(select(i, ...))
	end
	io.stderr:write(table.concat(parts, "\t"), "\n")
end

-- As the translation's host: files are read only (upstream would save its
-- settings next to its source)
local ioOpen = io.open
function io.open(path, mode)
	mode = mode or "r"
	if not mode:match("^r") or mode:match("%+") then
		return nil, path .. ": writing is not supported"
	end
	return ioOpen(path, mode)
end

headless.start()

-- Build codes: zlib streams, as SimpleGraphic's (headless upstream has no
-- implementation)
local ffi = require("ffi")
ffi.cdef[[
unsigned long compressBound(unsigned long sourceLen);
int compress2(uint8_t *dest, unsigned long *destLen, const uint8_t *source, unsigned long sourceLen, int level);
int uncompress(uint8_t *dest, unsigned long *destLen, const uint8_t *source, unsigned long sourceLen);
]]
local zlib = ffi.load("z")

function Deflate(data)
	local len = ffi.new("unsigned long[1]", zlib.compressBound(#data))
	local buf = ffi.new("uint8_t[?]", len[0])
	assert(zlib.compress2(buf, len, data, #data, 9) == 0, "Deflate failed")
	return ffi.string(buf, len[0])
end

function Inflate(data)
	local size = math.max(#data * 4, 1024)
	while size < 2 ^ 31 do
		local len = ffi.new("unsigned long[1]", size)
		local buf = ffi.new("uint8_t[?]", size)
		local r = zlib.uncompress(buf, len, data, #data)
		if r == 0 then
			return ffi.string(buf, len[0])
		elseif r ~= -5 then -- Z_BUF_ERROR: the buffer is too small
			return nil
		end
		size = size * 4
	end
end

local api = dofile(oracleDir .. "/../crates/session/lua/session.lua")
local first = api.handle('{"cmd":"newBuild"}')
assert(first:match('^{"ok"'), "newBuild failed: " .. first)

-- Not the engine API: `{"cmd":"oracleOutputs","xml":..,"id":..}` loads a
-- build as `dump.lua` does and answers its outputs in `dump.lua`'s JSON,
-- `{"ok":<outputs>}` (the variant tests' reference, see
-- crates/equivalence/src/oracle.rs); `{"cmd":"oracleLastError"}` answers
-- the last error with its source position, which the answers leave out
-- (the session test checks which upstream failure it is)
local toJSON = dofile(oracleDir .. "/json.lua")
local dkjson = require("dkjson")
local function oracleOutputs(req)
	local ok, res = pcall(function()
		loadBuildFromXML(req.xml, req.id or "variant")
		assert(build.calcsTab.mainEnv, "build did not calculate")
		return headless.outputs()
	end)
	if not ok then
		return dkjson.encode({ error = tostring(res) })
	end
	return '{"ok":' .. toJSON(res) .. '}'
end

-- `{"cmd":"oracleValue","fn":..,"params":..}`: one of values.lua's
-- functions, its result in values.lua's encoding (the reference of the
-- equivalence tests of items, trees and the data table)
local values = dofile(oracleDir .. "/values.lua")
local function oracleValue(req)
	local f = values[req.fn]
	if type(f) ~= "function" or req.fn == "encode" then
		return dkjson.encode({ error = "no oracle value " .. tostring(req.fn) })
	end
	local ok, res = pcall(function()
		return values.encode(f(req.params or { }))
	end)
	if not ok then
		return dkjson.encode({ error = tostring(res) })
	end
	return '{"ok":' .. res .. '}'
end

-- `{"cmd":"oracleCalculator","xml":..,"id":..,"cases":[..]}` loads a
-- build and answers its `calcsTab.miscCalculator`: the base outputs and
-- `calcFunc(override)`'s for each case, an override with nodes and items
-- by id (`addNodes`, `removeNodes`, `repSlotName`, `repItem`,
-- `toggleFlask`, `toggleTincture`, `conditions`), each actor's outputs as
-- `{player=..,minion=..}` (crates/equivalence/tests/calculator.rs)
local function oracleCalculator(req)
	local ok, res = pcall(function()
		loadBuildFromXML(req.xml, req.id or "calculator")
		local calcFunc, calcBase = build.calcsTab.miscCalculator[1], build.calcsTab.miscCalculator[2]
		local function actors(output)
			local player = { }
			for k, v in pairs(output) do
				if k ~= "Minion" then
					player[k] = v
				end
			end
			return { player = player, minion = output.Minion }
		end
		local nodes, items = build.spec.nodes, build.itemsTab.items
		local function set(ids)
			local s = { }
			for _, id in ipairs(ids) do
				s[nodes[id]] = true
			end
			return s
		end
		local answer = { base = actors(calcBase), cases = { } }
		for i, c in ipairs(req.cases) do
			local override = {
				addNodes = c.addNodes and set(c.addNodes),
				removeNodes = c.removeNodes and set(c.removeNodes),
				repSlotName = c.repSlotName,
				repItem = c.repItem and items[c.repItem],
				toggleFlask = c.toggleFlask and items[c.toggleFlask],
				toggleTincture = c.toggleTincture and items[c.toggleTincture],
				conditions = c.conditions,
			}
			local okCase, output = pcall(calcFunc, override)
			answer.cases[i] = okCase and actors(output) or { error = tostring(output) }
		end
		return answer
	end)
	if not ok then
		return dkjson.encode({ error = tostring(res) })
	end
	return '{"ok":' .. toJSON(res) .. '}'
end

-- `{"cmd":"oracleModWeights","xml":..,"id":..,"cases":[..]}` loads a
-- build and answers `TradeQueryGenerator`'s mod weights for each case:
-- `StartQuery(slot, options)` and its `ExecuteQuery`, with a slot of the
-- items tab by name and the options of the trade popup (`statWeights` as
-- `{stat, weightMult}` with the power stat's transform, `includeCorrupted`,
-- `includeTalisman`, `includeScourge`, `includeEldritch`, `jewelType`,
-- `special` by item name). Each group of mods is weighed an entry at a
-- time by key, one of the orders `pairs` gives (upstream weighs the first
-- of the entries sharing a trade mod id in `pairs` order, which LuaJIT
-- seeds per process). The weights are sorted by trade mod id
-- (crates/equivalence/tests/mod_weights.rs)
local function oracleModWeights(req)
	local ok, res = pcall(function()
		loadBuildFromXML(req.xml, req.id or "weights")
		local gen = new("TradeQueryGenerator"):TradeQueryGenerator({ itemsTab = build.itemsTab })
		local function byKey(f)
			return function(self, mods)
				local keys = { }
				for k in pairs(mods) do
					keys[#keys + 1] = k
				end
				table.sort(keys)
				for _, k in ipairs(keys) do
					f(self, { [k] = mods[k] })
				end
			end
		end
		gen.GenerateModWeights = byKey(gen.GenerateModWeights)
		gen.GeneratePassiveNodeWeights = byKey(gen.GeneratePassiveNodeWeights)
		local answer = { cases = { } }
		for i, c in ipairs(req.cases) do
			local okCase, result = pcall(function()
				local statWeights = { }
				for _, w in ipairs(c.statWeights) do
					local entry = { stat = w.stat, weightMult = w.weightMult }
					for _, s in ipairs(data.powerStatList) do
						if s.stat == w.stat then
							entry.label = s.label
							entry.transform = s.transform
						end
					end
					statWeights[#statWeights + 1] = entry
				end
				local options = {
					statWeights = statWeights,
					includeCorrupted = c.includeCorrupted,
					includeTalisman = c.includeTalisman,
					includeScourge = c.includeScourge,
					includeEldritch = c.includeEldritch,
					jewelType = c.jewelType,
					special = c.special and { itemName = c.special },
				}
				local slot = c.slot and build.itemsTab.slots[c.slot]
				assert(slot or not c.slot, "no slot " .. tostring(c.slot))
				gen.calcContext = { }
				gen:StartQuery(slot, options)
				local co = gen.calcContext.co
				assert(co, "no query for slot " .. tostring(c.slot))
				main:ClosePopup()
				while coroutine.status(co) ~= "dead" do
					local okCo, err = coroutine.resume(co, gen)
					if not okCo then
						error(err, 0)
					end
				end
				gen.calcContext.co = nil
				table.sort(gen.modWeights, function(a, b)
					return a.tradeModId < b.tradeModId
				end)
				return {
					category = gen.calcContext.itemCategory,
					baseStatValue = gen.calcContext.baseStatValue,
					weights = gen.modWeights,
				}
			end)
			answer.cases[i] = okCase and result or { error = tostring(result) }
		end
		return answer
	end)
	if not ok then
		return dkjson.encode({ error = tostring(res) })
	end
	return '{"ok":' .. toJSON(res) .. '}'
end

local lastError
for line in io.lines() do
	if line:match('^{"cmd":"oracleOutputs"') then
		out:write(oracleOutputs(dkjson.decode(line)), "\n")
		out:flush()
	elseif line:match('^{"cmd":"oracleCalculator"') then
		out:write(oracleCalculator(dkjson.decode(line)), "\n")
		out:flush()
	elseif line:match('^{"cmd":"oracleModWeights"') then
		out:write(oracleModWeights(dkjson.decode(line)), "\n")
		out:flush()
	elseif line:match('^{"cmd":"oracleValue"') then
		out:write(oracleValue(dkjson.decode(line)), "\n")
		out:flush()
	elseif line == '{"cmd":"oracleLastError"}' then
		out:write(dkjson.encode({ ok = lastError }), "\n")
		out:flush()
	elseif line:match("%S") then
		local raw = api.handle(line)
		if raw:match('^{"error"') then
			lastError = dkjson.decode(raw).error
		end
		-- The translation's errors carry no source positions
		local res = raw:gsub('^{"error":"[^"]-%.lua:%d+: ', '{"error":"')
		out:write(res, "\n")
		out:flush()
	end
end
