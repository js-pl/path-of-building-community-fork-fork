-- Baseline benchmark: times upstream headless on LuaJIT, the same calc code
-- desktop PoB runs, without rendering.
--
-- Run from upstream's src/ directory:
--   luajit <repo>/oracle/bench.lua <out file> <repeats> <id>=<build file>...
-- Usually run through oracle/run.py.
-- Times are CPU time (os.clock) in milliseconds; see oracle/README.md.

local outFile = assert(arg[1], "usage: bench.lua <out file> <repeats> <id>=<build file>...")
local repeats = assert(tonumber(arg[2]), "repeats must be a number")
local oracleDir = arg[0]:match("^(.*)/[^/]+$") or "."
local headless = dofile(oracleDir .. "/headless.lua")
local builds = headless.parseBuildArgs({ select(3, unpack(arg)) })

local function ms(seconds)
	return seconds * 1000
end

-- With the ModCache, like desktop PoB
local t0 = os.clock()
headless.start(true)
local startup = ms(os.clock() - t0)

local function stats(samples)
	table.sort(samples)
	local n = #samples
	local median = n % 2 == 1 and samples[(n + 1) / 2] or (samples[n / 2] + samples[n / 2 + 1]) / 2
	return { min = samples[1], median = median, max = samples[n], n = n }
end

local function timeIt(func)
	local samples = { }
	for i = 1, repeats do
		local t = os.clock()
		func()
		samples[i] = ms(os.clock() - t)
	end
	return stats(samples)
end

-- The work buildMode:OnFrame does after any build change (buildFlag branch)
local function recalc()
	wipeGlobalCache()
	build.outputRevision = build.outputRevision + 1
	build.skillsTab:UpdateSocketGroups()
	build.calcsTab:BuildOutput()
	build:RefreshStatList()
	build.configTab.calcFunc, build.configTab.calcBase = build.calcsTab:GetMiscCalculator(build)
end

-- Node power for every unallocated node, as the tree tab computes it, run to
-- completion outside the coroutine so it never yields
local function nodePower()
	build.calcsTab.powerMax = nil
	build.calcsTab:PowerBuilder()
end

local results = { }
for _, b in ipairs(builds) do
	local id = b.id
	local t = os.clock()
	headless.loadBuild(b.id, b.path)
	local load = ms(os.clock() - t)
	local result = {
		load = load,
		recalc = timeIt(recalc),
		nodePower = timeIt(nodePower),
	}
	results[id] = result
	print(string.format("%-30s load %8.1f  recalc %8.1f  node power %9.1f  (median ms)",
		id, load, result.recalc.median, result.nodePower.median))
end

-- Small JSON writer; values are numbers and strings only
local function toJSON(v, indent)
	indent = indent or ""
	if type(v) == "table" then
		local keys = { }
		for k in pairs(v) do
			keys[#keys + 1] = k
		end
		table.sort(keys)
		local parts = { }
		for _, k in ipairs(keys) do
			parts[#parts + 1] = indent .. "  " .. string.format("%q", k) .. ": " .. toJSON(v[k], indent .. "  ")
		end
		return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
	elseif type(v) == "number" then
		return v == math.floor(v) and string.format("%d", v) or string.format("%.3f", v)
	else
		return string.format("%q", tostring(v))
	end
end

local f = assert(io.open(outFile, "wb"))
f:write(toJSON({
	luajit = jit.version,
	clock = "cpu",
	repeats = repeats,
	startup = startup,
	builds = results,
}), "\n")
f:close()
