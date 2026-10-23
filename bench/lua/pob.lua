-- Boots upstream PoB headless and loads builds.
-- Must run with the working directory set to upstream/PathOfBuilding/src.

local ffi = require("ffi")

ffi.cdef[[
typedef struct { long tv_sec; long tv_nsec; } bench_timespec;
int clock_gettime(int clk_id, bench_timespec *tp);
unsigned long compressBound(unsigned long sourceLen);
int uncompress(uint8_t *dest, unsigned long *destLen, const uint8_t *source, unsigned long sourceLen);
]]

local pob = { }

local CLOCK_MONOTONIC = 1
local ts = ffi.new("bench_timespec")

-- Monotonic wall clock in milliseconds
function pob.clock()
	ffi.C.clock_gettime(CLOCK_MONOTONIC, ts)
	return tonumber(ts.tv_sec) * 1000 + tonumber(ts.tv_nsec) / 1e6
end

local zlib
local function inflate(data)
	zlib = zlib or ffi.load("z")
	local size = #data * 16
	while true do
		local buf = ffi.new("uint8_t[?]", size)
		local len = ffi.new("unsigned long[1]", size)
		local rc = zlib.uncompress(buf, len, data, #data)
		if rc == 0 then
			return ffi.string(buf, len[0])
		elseif rc == -5 then -- Z_BUF_ERROR
			size = size * 4
		else
			error("inflate failed: " .. rc)
		end
	end
end

function pob.read_file(path)
	local f = assert(io.open(path, "rb"))
	local text = f:read("*a")
	f:close()
	return text
end

-- Starts upstream headless. Upstream console output is silenced unless
-- verbose.
function pob.boot(verbose)
	local t0 = pob.clock()
	if not verbose then
		print = function() end
	end
	dofile("HeadlessWrapper.lua")
	-- Upstream's headless GetTime always returns 0
	GetTime = function() return math.floor(pob.clock()) end
	Inflate = inflate
	return pob.clock() - t0
end

-- Loads a build from a PoB XML file or a file holding a share code.
function pob.load_build(path)
	local t0 = pob.clock()
	local text = pob.read_file(path)
	if not text:match("^%s*<") then
		local code = text:gsub("%s", ""):gsub("-", "+"):gsub("_", "/")
		text = Inflate(common.base64.decode(code))
	end
	loadBuildFromXML(text, path:match("([^/]+)$"))
	assert(build.calcsTab.mainOutput, "build did not calculate: " .. path)
	return pob.clock() - t0
end

return pob
