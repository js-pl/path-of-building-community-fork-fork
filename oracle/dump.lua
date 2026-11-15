-- Oracle dumper: runs upstream headless and writes its calc outputs as JSON.
--
-- Run from upstream's src/ directory (upstream uses paths relative to it):
--   luajit <repo>/oracle/dump.lua <out dir> <id>=<build file>...
-- Build files are PoB XML (.xml) or PoE API character JSON (.json).
-- Usually run through oracle/run.py.
-- See oracle/README.md for the JSON format.

local outDir = assert(arg[1], "usage: dump.lua <out dir> <id>=<build file>...")
local oracleDir = arg[0]:match("^(.*)/[^/]+$") or "."
local headless = dofile(oracleDir .. "/headless.lua")
local builds = headless.parseBuildArgs({ select(2, unpack(arg)) })

headless.start()

local toJSON = dofile(oracleDir .. "/json.lua")

for _, b in ipairs(builds) do
	headless.loadBuild(b.id, b.path)
	local result = headless.outputs()
	local f = assert(io.open(outDir .. "/" .. b.id .. ".json", "wb"))
	f:write(toJSON(result), "\n")
	f:close()
	print("dumped " .. b.id)
end
