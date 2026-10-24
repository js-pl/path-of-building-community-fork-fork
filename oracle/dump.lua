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

-- Without the ModCache every mod line goes through ModParser, which is what
-- the port has to match and what coverage has to see
headless.start(false)

local toJSON = dofile(oracleDir .. "/json.lua")

-- player.output holds a reference to the minion's output table as Minion; it is
-- dumped separately as minion.output, so drop the alias here.
local function actorOutputs(env)
	local player = { }
	for k, v in pairs(env.player.output) do
		if not (k == "Minion" and env.minion and v == env.minion.output) then
			player[k] = v
		end
	end
	return {
		player = player,
		minion = env.minion and env.minion.output or nil,
	}
end

for _, b in ipairs(builds) do
	headless.loadBuild(b.id, b.path)
	local calcsTab = build.calcsTab
	local result = {
		MAIN = actorOutputs(calcsTab.mainEnv),
		CALCS = actorOutputs(calcsTab.calcsEnv),
	}
	local f = assert(io.open(outDir .. "/" .. b.id .. ".json", "wb"))
	f:write(toJSON(result), "\n")
	f:close()
	print("dumped " .. b.id)
end
