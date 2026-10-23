-- One headless upstream instance that runs a share of the power calc jobs.
--
-- usage: luajit worker.lua --build FILE --task node|anoint [--stat STAT]
--                          [--shard K --shards N | --upstream] --out FILE
--
-- --upstream runs upstream PowerBuilder unchanged (node task only) and records
-- every calc it makes. Otherwise the jobs are planned, sorted by key and every
-- N-th job starting at K is calculated.

local benchDir = arg[0]:match("^(.*)/[^/]+$") or "."
package.path = benchDir .. "/?.lua;" .. package.path

local pob = require("pob")
local power = require("power")

local opts = { stat = "combined", shard = 1, shards = 1 }
do
	local i = 1
	while i <= #arg do
		local name = arg[i]:match("^%-%-(.+)$")
		assert(name, "unexpected argument: " .. arg[i])
		if name == "upstream" then
			opts.upstream = true
			i = i + 1
		else
			opts[name] = assert(arg[i + 1], "missing value for --" .. name)
			i = i + 2
		end
	end
	opts.shard = tonumber(opts.shard)
	opts.shards = tonumber(opts.shards)
	assert(opts.build and opts.task and opts.out, "--build, --task and --out are required")
end

local out = assert(io.open(opts.out, "w"))
local function emit(...)
	out:write(table.concat({ ... }, "\t"), "\n")
end

local bootMs = pob.boot(os.getenv("BENCH_VERBOSE"))
local loadMs = pob.load_build(opts.build)
local calcsTab = build.calcsTab
emit("meta", "boot_ms", string.format("%.1f", bootMs))
emit("meta", "load_ms", string.format("%.1f", loadMs))

if opts.task == "node" then
	power.setPowerStat(calcsTab, opts.stat)
end

if opts.upstream then
	assert(opts.task == "node", "--upstream is only supported for the node task")
	local results, totalMs = power.runUpstreamNodePower(calcsTab)
	for _, result in ipairs(results) do
		emit("job", result.key, string.format("%.3f", result.ms), result.fingerprint)
	end
	emit("meta", "jobs", #results)
	emit("meta", "calc_ms", string.format("%.1f", totalMs))
else
	local t0 = pob.clock()
	local jobs
	if opts.task == "node" then
		jobs = power.planNodePower(calcsTab)
	elseif opts.task == "anoint" then
		jobs = power.planAnointPower(build)
	else
		error("unknown task: " .. opts.task)
	end
	if not jobs then
		emit("meta", "skipped", "no amulet")
		out:close()
		return
	end
	emit("meta", "plan_ms", string.format("%.1f", pob.clock() - t0))
	emit("meta", "jobs", #jobs)

	t0 = pob.clock()
	for i = opts.shard, #jobs, opts.shards do
		local job = jobs[i]
		local start = pob.clock()
		local output = job.run()
		emit("job", job.key, string.format("%.3f", pob.clock() - start), power.fingerprint(output))
	end
	emit("meta", "calc_ms", string.format("%.1f", pob.clock() - t0))
end
out:close()
