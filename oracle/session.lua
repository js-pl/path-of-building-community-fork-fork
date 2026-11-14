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

for line in io.lines() do
	if line:match("%S") then
		-- The translation's errors carry no source positions
		local res = api.handle(line):gsub('^{"error":"[^"]-%.lua:%d+: ', '{"error":"')
		out:write(res, "\n")
		out:flush()
	end
end
