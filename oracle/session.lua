-- The engine API on the oracle: api.lua on upstream headless on LuaJIT, the
-- reference of the session tests. JSON requests on stdin, one per line; one
-- JSON response per line on stdout, as `pob serve`.
--
-- Run from upstream's src/ directory (upstream uses paths relative to it):
--   luajit <repo>/oracle/session.lua

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

-- As the engine: files are read only (upstream would save its
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

local api = dofile(oracleDir .. "/api.lua")
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

local lastError
for line in io.lines() do
	if line:match('^{"cmd":"oracleOutputs"') then
		out:write(oracleOutputs(dkjson.decode(line)), "\n")
		out:flush()
	elseif line:match('^{"cmd":"oracleCalculator"') then
		out:write(oracleCalculator(dkjson.decode(line)), "\n")
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
		-- The typed engine's errors carry no source positions
		local res = raw:gsub('^{"error":"[^"]-%.lua:%d+: ', '{"error":"')
		out:write(res, "\n")
		out:flush()
	end
end
